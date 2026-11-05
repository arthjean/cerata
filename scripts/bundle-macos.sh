#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo build -p cerata --release
target_dir="$(cargo metadata --format-version 1 --no-deps | sed -E 's/.*"target_directory":"([^"]+)".*/\1/')"

app="$target_dir/release/Cerata.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target_dir/release/cerata" "$app/Contents/MacOS/cerata"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
cp crates/cerata/assets/brand/cerata.icns "$app/Contents/Resources/cerata.icns"

echo "$app"
