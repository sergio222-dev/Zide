#!/bin/sh
# Installs the latest Zide release to ~/.local/bin (or $ZIDE_BIN_DIR).
#   curl -fsSL https://raw.githubusercontent.com/sergio222-dev/Zide/main/scripts/install.sh | sh
# Pin a version with ZIDE_VERSION=0.1.0. The signature is checked when `minisign` is installed.
# Uninstall (removes the binary; add --purge to also delete settings, sessions, language
# servers and caches):
#   curl -fsSL https://raw.githubusercontent.com/sergio222-dev/Zide/main/scripts/install.sh | sh -s -- --uninstall [--purge]
set -eu

repo="sergio222-dev/Zide"
bin_dir="${ZIDE_BIN_DIR:-$HOME/.local/bin}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop_file="$data_home/applications/zide.desktop"
icon_file="$data_home/icons/hicolor/scalable/apps/zide.svg"
public_key="RWSTKtnMI9IrzUUoLyjrScQDHa0K2O3f4QEz6pcE+7/ucSlIIKLkWfGs"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin_dir/zide"
  rm -f "$desktop_file" "$icon_file"
  echo "Removed $bin_dir/zide and its launcher entry"
  if [ "${2:-}" = "--purge" ]; then
    rm -rf "$data_home/zide" "${XDG_CACHE_HOME:-$HOME/.cache}/zide"
    echo "Removed Zide data and cache."
  fi
  exit 0
fi

[ "$(uname -s)" = "Linux" ] && [ "$(uname -m)" = "x86_64" ] || {
  echo "zide: only Linux x86_64 is published" >&2
  exit 1
}

if [ -n "${ZIDE_VERSION:-}" ]; then
  version="$ZIDE_VERSION"
else
  version=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
    | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n 1)
fi
[ -n "$version" ] || { echo "zide: cannot find the latest version" >&2; exit 1; }

name="zide-$version-x86_64-unknown-linux-gnu"
url="https://github.com/$repo/releases/download/v$version/$name.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading Zide $version..."
curl -fsSL "$url" -o "$tmp/$name.tar.gz"

if command -v minisign >/dev/null 2>&1; then
  curl -fsSL "$url.minisig" -o "$tmp/$name.tar.gz.minisig"
  minisign -Vm "$tmp/$name.tar.gz" -P "$public_key" >/dev/null \
    || { echo "zide: signature check FAILED, nothing installed" >&2; exit 1; }
  echo "Signature OK."
else
  echo "minisign not found: signature not checked (the in-app updater always checks it)."
fi

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$bin_dir"
install -m 755 "$tmp/$name/zide" "$bin_dir/zide"
echo "Installed $bin_dir/zide"
# Launcher entry and icon (releases before 0.1.2 do not ship them).
if [ -f "$tmp/$name/zide.desktop" ] && [ -f "$tmp/$name/zide.svg" ]; then
  mkdir -p "$(dirname "$desktop_file")" "$(dirname "$icon_file")"
  install -m 644 "$tmp/$name/zide.svg" "$icon_file"
  sed "s|^Exec=zide |Exec=$bin_dir/zide |" "$tmp/$name/zide.desktop" > "$desktop_file"
  echo "Installed the launcher entry and icon"
fi
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "Add $bin_dir to your PATH." ;; esac
