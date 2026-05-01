use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct RouteBackup {
    vpn_server_ip: String,
}

pub struct RouteManager {
    backup_path: PathBuf,
    pinned_server_ip: Option<String>,
}

impl RouteManager {
    pub fn new(backup_path: PathBuf) -> Self {
        Self {
            backup_path,
            pinned_server_ip: None,
        }
    }

    /// In full-route mode the VPN's allowed_ips spans the entire IPv4
    /// space (0.0.0.0/1 + 128.0.0.0/1), so even wg's outer UDP packets
    /// destined for the VPN server itself get routed back into the
    /// tunnel — chicken-and-egg, the link goes dead. Install a host
    /// route for the VPN server's IP via the original IPv4 default
    /// gateway so wg's encapsulated packets bypass utun.
    pub fn pin_vpn_server(&mut self, server_ip: &str) -> Result<()> {
        let gateway = current_default_gateway()
            .context("no IPv4 default gateway, cannot pin VPN server route")?;
        log::info!("pinning host route for VPN server {server_ip} via {gateway}");

        // Try add; if it already exists, fall back to change.
        let add = Command::new("route")
            .args(["-n", "add", "-host", server_ip, &gateway])
            .status()
            .context("failed to spawn route add")?;
        if !add.success() {
            let chg = Command::new("route")
                .args(["-n", "change", "-host", server_ip, &gateway])
                .status()
                .context("failed to spawn route change")?;
            if !chg.success() {
                return Err(anyhow!(
                    "failed to install host route for {server_ip} via {gateway}"
                ));
            }
        }

        // Persist a marker so a crashed/SIGKILLed run leaves a
        // breadcrumb that the next start-up can recover from.
        let backup = RouteBackup {
            vpn_server_ip: server_ip.to_string(),
        };
        let data =
            serde_json::to_string_pretty(&backup).context("failed to serialize route backup")?;
        if let Some(parent) = self.backup_path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(&self.backup_path, data).with_context(|| {
            format!(
                "failed to write route backup {}",
                self.backup_path.display()
            )
        })?;

        self.pinned_server_ip = Some(server_ip.to_string());
        Ok(())
    }

    pub fn unpin(&mut self) {
        if let Some(ip) = self.pinned_server_ip.take() {
            let _ = Command::new("route")
                .args(["-n", "delete", "-host", &ip])
                .status();
            log::info!("removed host route for VPN server {ip}");
        }
        let _ = fs::remove_file(&self.backup_path);
    }

    /// On startup, look for a stale backup left by a previous run that
    /// didn't reach unpin() (panic / SIGKILL / hang). Drop the host
    /// route and the marker. Returns Ok(true) if a recovery was made.
    pub fn restore_from_stale_backup(path: &Path) -> Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed to read route backup {}", path.display()))?;
        let backup: RouteBackup =
            serde_json::from_str(&data).context("failed to parse route backup")?;
        if !backup.vpn_server_ip.is_empty() {
            let _ = Command::new("route")
                .args(["-n", "delete", "-host", &backup.vpn_server_ip])
                .status();
            log::info!(
                "removed stale VPN server host route for {}",
                backup.vpn_server_ip
            );
        }
        let _ = fs::remove_file(path);
        Ok(true)
    }
}

fn current_default_gateway() -> Option<String> {
    let output = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        line.trim()
            .strip_prefix("gateway:")
            .map(|s| s.trim().to_string())
    })
}
