#!/usr/bin/env bash
# Build the browser demo (web/ + sdv_core.wasm) into dist/ for Cloudflare Workers static assets.
#
# Workers Builds runs this as the build command; `npx wrangler deploy` then uploads dist/
# (see wrangler.jsonc). The Workers build image has no Rust toolchain, so a minimal one is
# installed with rustup when cargo is missing. Locally, an existing toolchain is reused.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT="$ROOT/dist"

if ! command -v cargo >/dev/null 2>&1; then
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    echo "== installing a minimal Rust toolchain (rustup)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
fi
rustup target add wasm32-unknown-unknown

echo "== building sdv_core.wasm"
cargo build --release --target wasm32-unknown-unknown --manifest-path "$ROOT/rust/sdv-core/Cargo.toml"

echo "== assembling $OUT"
rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$ROOT/web/." "$OUT"/
rm -f "$OUT/README.md"   # repository notes, not part of the site
cp "$ROOT/rust/sdv-core/target/wasm32-unknown-unknown/release/sdv_core.wasm" "$OUT"/
ls -l "$OUT"
