#!/usr/bin/env bash
# Build Ludolph.app from a binary: scripts/bundle.sh <binary> <version> <out-dir>
# Signs with the identity in LUDOLPH_SIGN_IDENTITY (from LUDOLPH_SIGN_KEYCHAIN if
# set), or ad hoc without one. macOS keeps the microphone and speech grants
# across updates only for a stable identity: an ad-hoc signature changes with
# every build.
set -euo pipefail

binary=$1
version=$2
out=$3

id=dev.iyazerski.ludolph
app="$out/Ludolph.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/ludolph"
cp assets/Ludolph.icns "$app/Contents/Resources/Ludolph.icns"
cp LICENSE THIRD_PARTY_LICENSES.md "$app/Contents/Resources/"
sed -e "s/@VERSION@/$version/g" assets/Info.plist >"$app/Contents/Info.plist"
codesign --force --sign "${LUDOLPH_SIGN_IDENTITY:--}" ${LUDOLPH_SIGN_KEYCHAIN:+--keychain "$LUDOLPH_SIGN_KEYCHAIN"} \
  --identifier "$id" "$app"
echo "$app"
