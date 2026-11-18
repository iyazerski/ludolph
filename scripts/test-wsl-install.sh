#!/usr/bin/env sh
set -eu
binary=$(realpath "$1")
version=$("$binary" --version)
work=$(mktemp -d)
engine_pid=
cleanup() {
  if [ -n "$engine_pid" ]; then
    kill "$engine_pid" 2>/dev/null || true
    wait "$engine_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$work/payload/ludolph/bin" "$work/payload/ludolph/share/ludolph/pi" "$work/assets" "$work/home/.pi/agent"
cp "$binary" "$work/payload/ludolph/bin/ludolph"
strip "$work/payload/ludolph/bin/ludolph"
printf 'bundled extension\n' > "$work/payload/ludolph/share/ludolph/pi/extension.js"
printf '{"keep":"my pi settings"}\n' > "$work/home/.pi/agent/settings.json"
tar -czf "$work/assets/ludolph-engine-linux-$(uname -m).tar.gz" -C "$work/payload" ludolph

HOME="$work/home" sh scripts/wsl-install.sh "$work/assets" "$version"
# An idle engine never calls pi. Stand in for that external executable at discovery only.
ln -s /bin/false "$work/home/.local/bin/pi"
HOME="$work/home" LUDOLPH_DIR="$work/state" PI_CODING_AGENT_DIR="$work/home/.pi/agent" \
  "$work/home/.local/bin/ludolph" engine >"$work/engine.log" 2>&1 &
engine_pid=$!
for attempt in $(seq 1 100); do
  [ -S "$work/state/engine.sock" ] && break
  if [ "$attempt" -eq 100 ]; then cat "$work/engine.log"; exit 1; fi
  sleep 0.1
done
HOME="$work/home" sh scripts/wsl-install.sh "$work/assets" "$version"
wait "$engine_pid"
engine_pid=
test "$("$work/home/.local/bin/ludolph" --version)" = "$version"
cmp "$work/payload/ludolph/share/ludolph/pi/extension.js" "$work/home/.local/share/ludolph/pi/extension.js"
test "$(cat "$work/home/.pi/agent/settings.json")" = '{"keep":"my pi settings"}'
if HOME="$work/home" sh scripts/wsl-install.sh "$work/assets" 999.0.0; then
  echo "Setup accepted a mismatched engine." >&2
  exit 1
fi
test "$("$work/home/.local/bin/ludolph" --version)" = "$version"
# The Windows runner uses the real Linux payload to exercise installer packaging.
mkdir -p target/wsl
cp "$work/assets/ludolph-engine-linux-$(uname -m).tar.gz" target/wsl/
