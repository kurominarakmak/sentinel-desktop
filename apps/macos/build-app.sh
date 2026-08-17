#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
configuration=${CONFIGURATION:-release}
output_root=${SENTINEL_APP_OUTPUT:-"$repository_root/target/native-app"}
app="$output_root/Agent Sentinel.app"
staging="$output_root/.Agent Sentinel.app.staging.$$"

cd "$repository_root"
cargo build --profile "$configuration" -p sentinel-native-bridge
swift build --package-path "$script_dir" -c "$configuration"

swift_bin=$(swift build --package-path "$script_dir" -c "$configuration" --show-bin-path)
rust_bin="$repository_root/target/$configuration/sentinel-native-bridge"

mkdir -p "$staging/Contents/MacOS" "$staging/Contents/Resources"
cp "$script_dir/Resources/Info.plist" "$staging/Contents/Info.plist"
cp "$repository_root/apps/desktop/src-tauri/icons/codex-tray.png" "$staging/Contents/Resources/codex-tray.png"
cp "$script_dir/Sources/SentinelMac/Resources/codex-mark.svg" "$staging/Contents/Resources/codex-mark.svg"
cp "$swift_bin/SentinelMac" "$staging/Contents/MacOS/SentinelMac"
cp "$rust_bin" "$staging/Contents/MacOS/sentinel-native-bridge"
chmod 755 "$staging/Contents/MacOS/SentinelMac" "$staging/Contents/MacOS/sentinel-native-bridge"
codesign --force --deep --sign - "$staging"
if [ -e "$app" ]; then
  /bin/rm -rf "$app"
fi
mv "$staging" "$app"
printf '%s\n' "$app"
