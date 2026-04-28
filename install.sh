#!/usr/bin/env bash
# corplink-rs (virxy fork) installer for macOS arm64
#   curl -fsSL https://raw.githubusercontent.com/virxy/corplink-rs/master/install.sh | bash
set -euo pipefail

REPO="virxy/corplink-rs"
ROOT="${CORPLINK_HOME:-$HOME/.corplink-rs}"
BIN_DIR="$HOME/.local/bin"

err() { echo "✗ $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || err "目前仅支持 macOS。Linux/Windows 见 README 自行编译。"
[[ "$(uname -m)" == "arm64"  ]] || err "目前仅支持 Apple Silicon (arm64)。Intel mac 需要自己编译。"

for c in curl tar jq fzf; do
    command -v "$c" >/dev/null 2>&1 || err "缺少依赖 $c,请先 'brew install $c'"
done

mkdir -p "$ROOT" "$BIN_DIR"

echo "→ 查询最新 release..."
TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | jq -r '.tag_name // empty')"
[[ -n "$TAG" ]] || err "未拿到 release tag,确认仓库 $REPO 已有 release"

ASSET="corplink-rs-${TAG}-macos-arm64.tar.gz"
URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"
echo "→ 下载 $URL"
curl -fL "$URL" -o "$ROOT/$ASSET" || err "下载失败"

echo "→ 解压到 $ROOT"
tar -xzf "$ROOT/$ASSET" -C "$ROOT"
rm "$ROOT/$ASSET"
chmod +x "$ROOT/corplink-rs" "$ROOT/corplink-tui"

echo "→ 创建 fl 命令"
ln -sf "$ROOT/corplink-tui" "$BIN_DIR/fl"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo
        echo "⚠  $BIN_DIR 不在 PATH 里。把下面这行加到 ~/.zshrc 或 ~/.bashrc:"
        echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
        echo "  然后重开终端,或先 source 一下。"
        ;;
esac

cat <<EOF

✓ 安装完成

启动:
    fl

首次会引导填公司代号 / 用户名 / 平台,然后用飞书扫码登录。
之后随时打 \`fl\` 切节点 / 看状态 / 断开。

文件:
    binary    $ROOT/corplink-rs
    config    $ROOT/config.json
    log       $ROOT/run.log
    cookies   $ROOT/<iface>_cookies.json
    fl        $BIN_DIR/fl  ->  $ROOT/corplink-tui

卸载:
    sudo pkill -INT -f corplink-rs
    rm -rf $ROOT $BIN_DIR/fl
EOF
