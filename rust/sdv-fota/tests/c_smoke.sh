#!/usr/bin/env bash
# Build and call the C bridge through its public header.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TERM_COLOR=never
LOG=$(cargo rustc -q --release --lib --features std --crate-type staticlib -- --print native-static-libs 2>&1)
LIBS=$(printf '%s\n' "$LOG" | sed -n 's/.*native-static-libs: //p' | tail -1)
LIB=target/release/libsdv_fota.a
OUT=target/c_smoke
# shellcheck disable=SC2086
${CC:-cc} -std=c11 -Wall -Wextra -Werror -Iinclude tests/c/smoke.c "$LIB" ${LIBS} -o "$OUT"
"$OUT"
