#!/usr/bin/env sh
# The Windows app passes its bundled payload folder, already mapped with wslpath.
set -eu

assets=$1
version=$2
case "$(uname -m)" in
  x86_64) arch=x86_64 ;;
  aarch64) arch=aarch64 ;;
  *) echo "This WSL architecture isn't supported." >&2; exit 1 ;;
esac

prefix="$HOME/.local"
mkdir -p "$prefix/bin" "$prefix/share"
work=$(mktemp -d "$prefix/.ludolph-install.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM
tar -xzf "$assets/ludolph-engine-linux-$arch.tar.gz" -C "$work"
new="$work/ludolph"
if [ "$("$new/bin/ludolph" --version)" != "$version" ]; then
  echo "The bundled engine doesn't match the Windows app." >&2
  exit 1
fi

# Only this user's installed engine, never other Ludolph builds or a whole WSL distribution.
engine_pids() {
  for pid in $(pgrep -u "$(id -u)" -x ludolph || true); do
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
    [ "${exe% (deleted)}" = "$prefix/bin/ludolph" ] || continue
    arg=$(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | awk 'NR == 2 {print; exit}')
    [ "$arg" = engine ] && printf '%s\n' "$pid"
  done
  return 0
}
pids=$(engine_pids)
if [ -n "$pids" ]; then
  # Intentional word splitting: each item is a PID from /proc.
  # shellcheck disable=SC2086
  kill $pids
fi
for attempt in $(seq 1 30); do
  [ -z "$(engine_pids)" ] && break
  if [ "$attempt" -eq 30 ]; then
    echo "The engine is still stopping. Try again once running chats have stopped." >&2
    exit 1
  fi
  sleep 1
done

# Stage on the same filesystem. Keep the old pair until both replacements succeed.
restore() {
  status=$?
  if [ "$status" -ne 0 ]; then
    if [ -e "$work/old-bin" ]; then mv -f "$work/old-bin" "$prefix/bin/ludolph"; fi
    if [ -d "$work/old-share" ]; then
      rm -rf "$prefix/share/ludolph"
      mv "$work/old-share" "$prefix/share/ludolph"
    fi
  fi
  rm -rf "$work"
  exit "$status"
}
trap restore EXIT
if [ -e "$prefix/bin/ludolph" ]; then cp -p "$prefix/bin/ludolph" "$work/old-bin"; fi
if [ -d "$prefix/share/ludolph" ]; then mv "$prefix/share/ludolph" "$work/old-share"; fi
mv "$new/share/ludolph" "$prefix/share/ludolph"
mv -f "$new/bin/ludolph" "$prefix/bin/ludolph"
