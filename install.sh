#!/usr/bin/env sh
set -eu

repo="iyazerski/ludolph"
asset="Ludolph.zip"
id="dev.iyazerski.ludolph"
# The checksum detects damage; this certificate authenticates releases. Keep it in sync with the updater.
leaf="7c6d7b2fa7382af0dd7f9f3e31f6e016fbe2fd46"
signer="=identifier \"$id\" and certificate leaf = H\"$leaf\""

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) os=macos ;;
  Linux-x86_64) os=linux asset="ludolph-linux-x86_64.tar.gz" ;;
  Linux-aarch64) os=linux asset="ludolph-linux-aarch64.tar.gz" ;;
  *)
    echo "Ludolph supports macOS on Apple silicon, and Linux on x86_64 or ARM64." >&2
    exit 1
    ;;
esac
if ! command -v pi >/dev/null 2>&1 && [ ! -x "$HOME/.bun/bin/pi" ] && [ ! -x "$HOME/.local/bin/pi" ]; then
  echo "Ludolph runs pi agents, and pi isn't installed. Get it from https://pi.dev first." >&2
fi

if [ -n "${LUDOLPH_VERSION:-}" ]; then
  download_url="https://github.com/$repo/releases/download/$LUDOLPH_VERSION/$asset"
else
  download_url="https://github.com/$repo/releases/latest/download/$asset"
fi
install_tmp="$(mktemp -d)"
trap 'rm -rf "$install_tmp"' EXIT HUP INT TERM

echo "Downloading $asset..."
curl -fsSL "$download_url" -o "$install_tmp/$asset"
curl -fsSL "$download_url.sha256" -o "$install_tmp/$asset.sha256"

if [ "$os" = linux ]; then
  # Linux releases aren't signed: the checksum, fetched over HTTPS like this script, only
  # detects damage.
  (cd "$install_tmp" && sha256sum -c "$asset.sha256")
  prefix="$HOME/.local"
  tar -xzf "$install_tmp/$asset" -C "$install_tmp"
  mkdir -p "$prefix/bin" "$prefix/share"
  # The running engine is the old version: stop it, and the next `ludolph` starts the new one.
  pkill -f "$prefix/bin/ludolph engine" 2>/dev/null || true
  rm -rf "$prefix/share/ludolph"
  mv "$install_tmp/ludolph/share/ludolph" "$prefix/share/ludolph"
  mv "$install_tmp/ludolph/bin/ludolph" "$prefix/bin/ludolph"
  echo "Ludolph is installed to $prefix/bin/ludolph."
  # The engine shares the binary with the app, so it needs the app's libraries too.
  missing="$(ldd "$prefix/bin/ludolph" 2>/dev/null | sed -n 's/^[[:space:]]*\([^ ]*\) => not found$/\1/p')"
  if [ -n "$missing" ]; then
    echo "Ludolph needs $(echo "$missing" | tr '\n' ' ')to run. Install them, for example with: sudo apt install libxkbcommon0" >&2
    exit 1
  fi
  if grep -qi microsoft /proc/version 2>/dev/null; then
    echo "In Windows, get Ludolph.exe from https://github.com/$repo/releases/latest and open it."
  else
    echo "Run ludolph to open it."
  fi
  exit 0
fi

(cd "$install_tmp" && shasum -a 256 -c "$asset.sha256")
ditto -x -k "$install_tmp/$asset" "$install_tmp"
if ! codesign --verify --strict -R "$signer" "$install_tmp/Ludolph.app"; then
  echo "The download isn't signed by Ludolph's release certificate. Not installing it." >&2
  exit 1
fi

apps="/Applications"
if [ ! -w "$apps" ]; then
  apps="$HOME/Applications"
  mkdir -p "$apps"
fi

binary="$apps/Ludolph.app/Contents/MacOS/ludolph"
pkill -f "$binary" 2>/dev/null || true
i=0
while pgrep -qf "$binary" && [ $i -lt 150 ]; do
  sleep 0.1
  i=$((i + 1))
done
rm -rf "$apps/Ludolph.app"
mv "$install_tmp/Ludolph.app" "$apps/Ludolph.app"
xattr -dr com.apple.quarantine "$apps/Ludolph.app" 2>/dev/null || true

echo "Ludolph $(defaults read "$apps/Ludolph.app/Contents/Info" CFBundleShortVersionString) is installed to $apps/Ludolph.app"
open "$apps/Ludolph.app"
echo "Ludolph is running. Press ⌃⌥Space for the quick window."
