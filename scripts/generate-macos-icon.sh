#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
icon_src="$root/assets/icons/1.png"
icon_dst="$root/assets/icons/AppIcon.icns"
iconset_dir="$(mktemp -d)"
iconset_path="$iconset_dir/AppIcon.iconset"

cleanup() {
  rm -rf "$iconset_dir"
}
trap cleanup EXIT

if [[ ! -f "$icon_src" ]]; then
  echo "Missing app icon source: $icon_src" >&2
  exit 1
fi

mkdir -p "$iconset_path"

sips -z 16 16 "$icon_src" --out "$iconset_path/icon_16x16.png" >/dev/null
sips -z 32 32 "$icon_src" --out "$iconset_path/icon_16x16@2x.png" >/dev/null
sips -z 32 32 "$icon_src" --out "$iconset_path/icon_32x32.png" >/dev/null
sips -z 64 64 "$icon_src" --out "$iconset_path/icon_32x32@2x.png" >/dev/null
sips -z 128 128 "$icon_src" --out "$iconset_path/icon_128x128.png" >/dev/null
sips -z 256 256 "$icon_src" --out "$iconset_path/icon_128x128@2x.png" >/dev/null
sips -z 256 256 "$icon_src" --out "$iconset_path/icon_256x256.png" >/dev/null
sips -z 512 512 "$icon_src" --out "$iconset_path/icon_256x256@2x.png" >/dev/null
sips -z 512 512 "$icon_src" --out "$iconset_path/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$icon_src" --out "$iconset_path/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$iconset_path" -o "$icon_dst"

echo "Generated macOS app icon: $icon_dst"
