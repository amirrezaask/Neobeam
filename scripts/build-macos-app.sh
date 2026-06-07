#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if ! command -v cargo-bundle >/dev/null 2>&1; then
  echo "cargo-bundle is not installed. Install it with:" >&2
  echo "  cargo install cargo-bundle" >&2
  exit 1
fi

cargo bundle --release -p neobeam --format osx

app_path="$root/target/release/bundle/osx/Neobeam.app"
echo "Built macOS app bundle: $app_path"
