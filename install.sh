#!/usr/bin/env sh
# Install or update Ludolph.app from the latest release, and start it.
# Set LUDOLPH_VERSION=vX.Y.Z to pin a release.
set -eu

repo="iyazerski/ludolph"
asset="Ludolph.zip"
id="dev.iyazerski.ludolph"
# Every release is signed with this certificate; the checksum only catches
# damage. The release workflow checks its builds against this line too.
leaf="7c6d7b2fa7382af0dd7f9f3e31f6e016fbe2fd46"
signer="=identifier \"$id\" and certificate leaf = H\"$leaf\""

if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]; then
  echo "Ludolph supports macOS on Apple silicon only." >&2
  exit 1
fi
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
(cd "$install_tmp" && shasum -a 256 -c "$asset.sha256")
ditto -x -k "$install_tmp/$asset" "$install_tmp"
if ! codesign --verify --strict -R "$signer" "$install_tmp/Ludolph.app"; then
  echo "The download isn't signed by Ludolph's release certificate. Not installing it." >&2
  exit 1
fi

# /Applications when it's writable without sudo, else ~/Applications.
apps="/Applications"
if [ ! -w "$apps" ]; then
  apps="$HOME/Applications"
  mkdir -p "$apps"
fi

# Stop the running app and its engine. The engine gives its pi processes up
# to 10 s to stop.
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
# The app starts its engine again.
open "$apps/Ludolph.app"
echo "Ludolph is running. Press ⌃⌥Space for the quick window."
