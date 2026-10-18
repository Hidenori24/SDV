#!/usr/bin/env bash
# Build the library as a static library (with std, so a panic handler exists) and call it from C through the header.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo rustc -q --release --features std --crate-type staticlib
LIB=target/release/libsdv_sec.a
LIBS=$(cargo rustc -q --release --features std --crate-type staticlib -- --print native-static-libs 2>&1 | sed -n 's/.*native-static-libs: //p' | tail -1)
OUT=target/c_smoke
${CC:-cc} -std=c11 -Wall -Wextra -Werror -Iinclude tests/c/smoke.c "$LIB" ${LIBS} -o "$OUT"
"$OUT"
