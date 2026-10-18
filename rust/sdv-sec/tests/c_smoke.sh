#!/usr/bin/env bash
# Build the library as a static library (with std, so a panic handler exists) and call it from C through the header.
set -euo pipefail
cd "$(dirname "$0")/.."
# CI sets CARGO_TERM_COLOR=always; colour codes would end up in the linker arguments below.
export CARGO_TERM_COLOR=never
LOG=$(cargo rustc -q --release --features std --crate-type staticlib -- --print native-static-libs 2>&1)
LIBS=$(printf '%s\n' "$LOG" | sed -n 's/.*native-static-libs: //p' | tail -1)
LIB=target/release/libsdv_sec.a
OUT=target/c_smoke
# shellcheck disable=SC2086  # LIBS is a list of linker flags
${CC:-cc} -std=c11 -Wall -Wextra -Werror -Iinclude tests/c/smoke.c "$LIB" ${LIBS} -o "$OUT"
"$OUT"
