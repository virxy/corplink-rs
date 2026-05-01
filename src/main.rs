mod api;
mod client;
mod config;
mod dns;
mod qrcode;
mod resp;
#[cfg(target_os = "macos")]
mod route;
mod state;
mod template;
mod totp;
mod utils;
mod wg;

#[cfg(windows)]
use is_elevated;

#[cfg(target_os = "macos")]
use dns::DNSManager;
#[cfg(target_os = "macos")]
use route::RouteManager;

use std::env;
use std::process::exit;

use anyhow::{anyhow, Context, Result};

use client::Client;
use config::{Config, WgConf};

struct CliArgs {
    conf_file: String,
    list_only: bool,
    login_only: bool,
    server_override: Option<String>,
}

fn print_usage_and_exit(name: &str) -> ! {
    println!(
        "usage:\n\t{name} [--list] [--login-only] [--server NAME] [config.json]\n\n\
         options:\n\
         \t--list           login, print available vpn nodes as TSV (name<TAB>ip<TAB>latency_ms), then exit\n\
         \t--login-only     run login flow (Lark scan etc), persist cookies, then exit\n\
         \t--server NAME    override vpn_server_name from config\n"
    );
    exit(1);
}

fn parse_arg() -> CliArgs {
    let mut conf_file = String::from("config.json");
    let mut list_only = false;
    let mut login_only = false;
    let mut server_override: Option<String> = None;
    let mut args = env::args();
    let name = args.next().unwrap();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => print_usage_and_exit(&name),
            "--list" => list_only = true,
            "--login-only" => login_only = true,
            "--server" => match args.next() {
                Some(v) => server_override = Some(v),
                None => print_usage_and_exit(&name),
            },
            _ if arg.starts_with("--") => print_usage_and_exit(&name),
            _ => conf_file = arg,
        }
    }
    CliArgs {
        conf_file,
        list_only,
        login_only,
        server_override,
    }
}

pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ETIMEDOUT: i32 = 110;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        log::error!("{:#}", err);
        exit(EPERM);
    }
}

async fn run() -> Result<()> {
    // NOTE: If you want to debug, you should set `RUST_LOG` env to `debug` and run corplink-rs in root
    //  because `check_privilege` will call sudo and drop env if you're not root
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    print_version();
    check_privilege();

    let cli = parse_arg();

    // Both backup paths are computed eagerly so the DNS/Route managers can
    // share them later in the connect path. The actual stale-backup
    // recovery is deferred until we know we're going to establish a
    // connection (NOT for --list / --login-only, which are auxiliary
    // commands and would otherwise mistake a healthy tunnel's still-live
    // backup files for stale leftovers — undoing the live tunnel's DNS
    // and host route mid-flight).
    #[cfg(target_os = "macos")]
    let dns_backup_path: std::path::PathBuf = {
        let conf_path = std::path::Path::new(&cli.conf_file);
        let conf_dir = conf_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        conf_dir.join("dns_backup.json")
    };
    #[cfg(target_os = "macos")]
    let route_backup_path: std::path::PathBuf = {
        let conf_path = std::path::Path::new(&cli.conf_file);
        let conf_dir = conf_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        conf_dir.join("route_backup.json")
    };

    let mut conf = Config::from_file(&cli.conf_file)
        .await
        .context("failed to load config")?;
    if let Some(server) = &cli.server_override {
        conf.vpn_server_name = Some(server.clone());
    }
    let name = conf
        .interface_name
        .clone()
        .context("interface name missing in config")?;

    #[cfg(target_os = "macos")]
    let use_vpn_dns = conf.use_vpn_dns.unwrap_or(false);
    #[cfg(target_os = "macos")]
    let use_full_route = conf.use_full_route.unwrap_or(false);

    if conf.server.is_none() {
        let resp = client::get_company_url(conf.company_name.as_str())
            .await
            .with_context(|| {
                format!(
                    "failed to fetch company server from company name {}",
                    conf.company_name
                )
            })?;
        log::info!(
            "company name is {}(zh)/{}(en) server is {}",
            resp.zh_name,
            resp.en_name,
            resp.domain
        );
        conf.server = Some(resp.domain);
        conf.save()
            .await
            .context("failed to persist company server")?;
    }

    let with_wg_log = conf.debug_wg.unwrap_or_default();
    let mut c = Client::new(conf).context("failed to initialize client")?;
    let mut logout_retry = true;
    let wg_conf: Option<WgConf>;

    if cli.login_only {
        if c.need_login() {
            log::info!("not login yet, try to login");
            c.login().await.context("login failed")?;
            log::info!("login success");
        } else {
            log::info!("already logged in");
        }
        return Ok(());
    }

    if cli.list_only {
        if c.need_login() {
            log::info!("not login yet, try to login");
            c.login().await.context("login failed")?;
            log::info!("login success");
        }
        let vpns = c.list_vpn().await.context("failed to list vpn")?;
        for v in &vpns {
            let latency = match c.ping_vpn(v.ip.clone(), v.api_port).await {
                Ok(l) => l,
                Err(_) => -1,
            };
            // TSV: name <TAB> ip <TAB> latency_ms (or -1 timeout)
            println!("{}\t{}\t{}", v.name, v.ip, latency);
        }
        return Ok(());
    }

    // Now that we know this is the real connect flow (not --list /
    // --login-only), recover any stale backups left by a previous
    // crashed run. Doing this earlier would clobber a live tunnel's
    // backup files when an auxiliary command runs alongside it.
    #[cfg(target_os = "macos")]
    match DNSManager::restore_from_stale_backup(&dns_backup_path) {
        Ok(true) => log::warn!(
            "recovered DNS from stale backup at {} — previous run did not clean up",
            dns_backup_path.display()
        ),
        Ok(false) => {}
        Err(e) => log::warn!("failed to restore stale dns backup: {}", e),
    }
    #[cfg(target_os = "macos")]
    match RouteManager::restore_from_stale_backup(&route_backup_path) {
        Ok(true) => log::warn!(
            "recovered VPN server host route from stale backup at {} — previous run did not clean up",
            route_backup_path.display()
        ),
        Ok(false) => {}
        Err(e) => log::warn!("failed to restore stale route backup: {}", e),
    }

    loop {
        if c.need_login() {
            log::info!("not login yet, try to login");
            c.login().await.context("login failed")?;
            log::info!("login success");
        }
        log::info!("try to connect");
        match c.connect_vpn().await {
            Ok(conf) => {
                wg_conf = Some(conf);
                break;
            }
            Err(e) => {
                if logout_retry && e.to_string().contains("logout") {
                    // e contains detail message, so just print it out
                    log::warn!("{}", e);
                    logout_retry = false;
                    continue;
                } else {
                    return Err(e);
                }
            }
        };
    }
    log::info!("start wg-corplink for {}", &name);
    let wg_conf = wg_conf.ok_or_else(|| anyhow!("wg conf missing after connect loop"))?;
    let protocol = wg_conf.protocol;
    wg::start_wg_go(&name, protocol, with_wg_log)
        .with_context(|| format!("failed to start wg-corplink for {}", name))?;
    let mut uapi = wg::UAPIClient { name: name.clone() };
    uapi.config_wg(&wg_conf)
        .await
        .with_context(|| format!("failed to config interface with uapi for {name}"))?;

    #[cfg(target_os = "macos")]
    let mut dns_manager = DNSManager::new(dns_backup_path.clone());
    #[cfg(target_os = "macos")]
    let mut route_manager = RouteManager::new(route_backup_path.clone());

    #[cfg(target_os = "macos")]
    if use_vpn_dns {
        match dns_manager.set_dns(vec![&wg_conf.dns], vec![]) {
            Ok(_) => {}
            Err(err) => {
                log::warn!("failed to set dns: {}", err);
            }
        }
    }

    // Full-route mode covers 0.0.0.0/0 — even the wg outer UDP packets
    // would loop back into utun and the link dies. Pin a host route for
    // the VPN server's IP via the original default gateway.
    #[cfg(target_os = "macos")]
    if use_full_route {
        let server_ip = wg_conf
            .peer_address
            .split(':')
            .next()
            .unwrap_or(&wg_conf.peer_address)
            .to_string();
        if let Err(err) = route_manager.pin_vpn_server(&server_ip) {
            log::warn!("failed to pin VPN server host route: {}", err);
        }
    }

    let mut exit_code = 0;
    tokio::select! {
        // handle signal
        _ = async {
            match tokio::signal::ctrl_c().await {
                Ok(_) => {},
                Err(e) => {
                    log::warn!("failed to receive signal: {}",e);
                },
            }
            log::info!("ctrl+c received");
        } => {},

        // keep alive
        // _ = c.keep_alive_vpn(&wg_conf, 60) => {
        //     exit_code = ETIMEDOUT;
        // },

        // check wg handshake and exit if timeout
        _ = async {
            uapi.check_wg_connection().await;
            log::warn!("last handshake timeout");
        } => {
            exit_code = ETIMEDOUT;
        },
    }

    // shutdown
    log::info!("disconnecting vpn...");
    if let Err(e) = c.disconnect_vpn(&wg_conf).await {
        log::warn!("failed to disconnect vpn: {}", e)
    };

    wg::stop_wg_go();

    #[cfg(target_os = "macos")]
    if use_vpn_dns {
        match dns_manager.restore_dns() {
            Ok(_) => {}
            Err(err) => {
                log::warn!("failed to delete dns: {}", err);
            }
        }
    }

    #[cfg(target_os = "macos")]
    route_manager.unpin();

    log::info!("reach exit");
    exit(exit_code)
}

fn check_privilege() {
    #[cfg(unix)]
    match sudo::escalate_if_needed() {
        Ok(_) => {}
        Err(_) => {
            log::error!("please run as root");
            exit(EPERM);
        }
    }

    #[cfg(windows)]
    if !is_elevated::is_elevated() {
        log::error!("please run as administrator");
        exit(EPERM);
    }
}

fn print_version() {
    let pkg_name = env!("CARGO_PKG_NAME");
    let pkg_version = env!("CARGO_PKG_VERSION");
    log::info!("running {}@{}", pkg_name, pkg_version);
}
