#!/usr/bin/env bash
# First-day check for a Raspberry Pi (or any Linux box): is this machine ready to run the SDV software?
#
#   tools/pi/bringup_check.sh            # everything (build, all tests, FOTA dry run)
#   tools/pi/bringup_check.sh --quick    # skip the test suites (build + FOTA dry run only)
#
# What it does, in order:
#   1. reports the environment (model, OS, 64-bit, glibc, memory, I2C / GPIO devices)
#   2. builds sdv-fota and sdv-picarx
#   3. runs the test suites of all four crates natively
#   4. FOTA dry run in a temporary directory: sign -> install -> trial boot -> commit,
#      then a tampered package, a downgrade and a failing health check (rollback)
#   5. lists the I2C / GPIO devices with sdv-picarx-discover
#
# What it does NOT do: it never uses sudo, never writes outside the repository's target/ directories and one
# temporary directory, never opens a GPIO line or an I2C device for writing, and never moves an actuator.
# The signing key it makes is a throw-away test key that lives in the temporary directory only.
set -uo pipefail

QUICK=0
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    -h | --help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
export CARGO_TERM_COLOR=never
WORK=$(mktemp -d "${TMPDIR:-/tmp}/sdv-bringup.XXXXXX")
LOG="$WORK/logs"
mkdir -p "$LOG"
RESULTS=()
FAILED=0

note() { printf '%s\n' "$*"; }
record() { # record <PASS|FAIL|SKIP|INFO> <seconds> <name> [detail]
  RESULTS+=("$(printf '%-4s %5ss  %s%s' "$1" "$2" "$3" "${4:+  ($4)}")")
  [ "$1" = FAIL ] && FAILED=$((FAILED + 1))
  return 0
}
step() { # step <name> <command...>: run, time and record a command; its output goes to a log file
  local name=$1; shift
  local start=$SECONDS file="$LOG/${name// /_}.log"
  printf '... %s\n' "$name"
  if "$@" >"$file" 2>&1; then record PASS $((SECONDS - start)) "$name"
  else record FAIL $((SECONDS - start)) "$name" "see $file"; tail -n 15 "$file" | sed 's/^/      /'; fi
}

# ---------------------------------------------------------------- 1. environment
note "== 1. environment"
ARCH=$(uname -m)
MODEL="unknown (not a Raspberry Pi, or no device tree)"
[ -r /proc/device-tree/model ] && MODEL=$(tr -d '\0' </proc/device-tree/model)
OS=$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-unknown}")
GLIBC=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
MEM_MB=$(awk '/MemTotal/ {printf "%d", $2/1024}' /proc/meminfo)
DISK_FREE=$(df -h --output=avail "$ROOT" | tail -1 | tr -d ' ')
note "  model      : $MODEL"
note "  os         : $OS"
note "  kernel/arch: $(uname -r) / $ARCH ($(getconf LONG_BIT)-bit userland)"
note "  glibc      : ${GLIBC:-unknown}"
note "  memory     : ${MEM_MB} MB, free disk here: $DISK_FREE"
note "  user       : $(id -un) (uid $(id -u)), groups: $(id -Gn)"
note "  i2c devices: $(ls /dev/i2c-* 2>/dev/null | tr '\n' ' ' || true)$([ -e /dev/i2c-1 ] || echo '(none: enable I2C with raspi-config before connecting the Robot HAT)')"
note "  gpio chips : $(ls /dev/gpiochip* 2>/dev/null | tr '\n' ' ')"
note "  rust       : $(rustc --version 2>/dev/null || echo 'NOT INSTALLED')"
note "  cc         : $(cc --version 2>/dev/null | head -1 || echo 'NOT INSTALLED')"

if [ "$ARCH" = aarch64 ] && [ "$(getconf LONG_BIT)" = 64 ]; then record PASS 0 "64-bit aarch64 system"
elif [ "$ARCH" = aarch64 ]; then record FAIL 0 "64-bit aarch64 system" "the kernel is 64-bit but the userland is 32-bit: install the 64-bit OS"
elif [ "$ARCH" = armv7l ] || [ "$ARCH" = armv6l ]; then record FAIL 0 "64-bit aarch64 system" "this is a 32-bit OS ($ARCH): install the 64-bit Raspberry Pi OS"
else record INFO 0 "not a Pi ($ARCH): the checks still run, but this is not the real target"; fi

# The aarch64 binaries CI builds need glibc >= 2.39 (issue #187). Building here, as this script does, has no such limit.
if [ -n "$GLIBC" ] && [ "$ARCH" = aarch64 ]; then
  if [ "$(printf '%s\n2.39\n' "$GLIBC" | sort -V | head -1)" = 2.39 ]; then record INFO 0 "glibc $GLIBC: the prebuilt CI binaries (sdv-aarch64-binaries) can run here"
  else record INFO 0 "glibc $GLIBC < 2.39: the prebuilt CI binaries will NOT start here; build on this machine (this script does)"; fi
fi

if ! command -v cargo >/dev/null || ! command -v cc >/dev/null; then
  record FAIL 0 "toolchain" "need cargo (https://rustup.rs) and a C compiler (sudo apt install build-essential)"
  printf '\n== summary\n'; printf '  %s\n' "${RESULTS[@]}"; exit 1
fi

# ---------------------------------------------------------------- 2. build
note "== 2. build (the first build downloads and compiles the dependencies; this is the slow part)"
# Debug builds: the FOTA dry run needs the development trust store, which release builds refuse on purpose.
step "build sdv-fota" cargo build --manifest-path rust/sdv-fota/Cargo.toml --features host-signing
step "build sdv-picarx" cargo build --manifest-path rust/sdv-picarx/Cargo.toml

# ---------------------------------------------------------------- 3. tests
if [ "$QUICK" = 1 ]; then
  record SKIP 0 "test suites" "--quick"
else
  note "== 3. tests (natively on this machine)"
  step "test sdv-sec" cargo test --manifest-path rust/sdv-sec/Cargo.toml --features sign
  step "test sdv-core" cargo test --manifest-path rust/sdv-core/Cargo.toml
  step "test sdv-fota" cargo test --manifest-path rust/sdv-fota/Cargo.toml
  # These spawn worker processes and check 10 ms cycle deadlines: CI only compiles them for aarch64, because
  # emulation cannot keep the deadlines. A real Pi is the first place they run on the real architecture.
  step "test sdv-picarx" cargo test --manifest-path rust/sdv-picarx/Cargo.toml
fi

# ---------------------------------------------------------------- 4. FOTA dry run
note "== 4. FOTA dry run (temporary directory, throw-away test key, nothing installed on the system)"
FOTA_BIN="$ROOT/rust/sdv-fota/target/debug"
cat >"$WORK/fota_dry_run.sh" <<'DRYRUN'
# Runs as a non-root user: the launcher refuses to start an application as root.
set -u
BIN=$1; W=$2
cd "$W"; umask 077
fail() { echo "FAILED: $*"; exit 1; }
status() { "$BIN/sdv-fota" status --state-root "$W/state"; }
sign() { # sign <version> <security-version> <image> <output>: prints the public key
  "$BIN/sdv-fota-sign" --seed seed.hex --key-id bringup --target vehicle --version "$1" --security-version "$2" \
    --trial-ms 1500 --config-schema 2 --signal-if-version 1 --image "$3" --output "$4" | sed -n 's/^public_key: //p'
}
install_pkg() { "$BIN/sdv-fota" install --state-root "$W/state" --package "$1" --target vehicle --config-schema 2 --signal-if-version 1 --trial-boots 1; }

head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >seed.hex
mkdir -p state/slots/a state/slots/b && chmod 755 state state/slots state/slots/a state/slots/b
printf '#!/bin/sh\nexec sleep 3\n' >state/slots/a/sdv-image.bin && chmod 755 state/slots/a/sdv-image.bin
"$BIN/sdv-fota" init --state-root "$W/state" --active-slot a --active-version 1.0.0 --security-version 1 || fail "init"

printf '#!/bin/sh\necho "image 1.1.0 runs as uid $(id -u)"\nexec sleep 3\n' >v110.sh
PUB=$(sign 1.1.0 2 v110.sh v110.sdvf) || fail "sign 1.1.0"
printf 'bringup %s\n' "$PUB" >trusted-keys && chmod 644 trusted-keys
export SDV_FOTA_TEST_TRUST_STORE="$W/trusted-keys" SDV_FOTA_APP_UID="$(id -u)" SDV_FOTA_APP_GID="$(id -g)"

echo "--- a signed update is staged as a trial"
install_pkg v110.sdvf || fail "install 1.1.0"
status | grep -q '^trial_version: 1.1.0' || fail "1.1.0 should be the pending trial"
status | grep -q '^active_version: 1.0.0' || fail "1.0.0 should still be active before the trial boot"

echo "--- trial boot with a passing health check commits it"
"$BIN/sdv-fota-launcher" "$W/state" /bin/true || fail "launcher (health ok)"
status | grep -q '^active_version: 1.1.0' || fail "1.1.0 should be active after the commit"
status | grep -q '^last_trial_outcome: committed' || fail "the trial should be recorded as committed"

echo "--- a tampered package is rejected"
printf '#!/bin/sh\nexec sleep 3\n' >v120.sh
sign 1.2.0 3 v120.sh v120.sdvf >/dev/null || fail "sign 1.2.0"
# flip one byte of the signed manifest (always to a different value)
cp v120.sdvf tampered.sdvf
[ "$(dd if=tampered.sdvf bs=1 skip=120 count=1 2>/dev/null)" = X ] && FLIP=Y || FLIP=X
printf '%s' "$FLIP" | dd of=tampered.sdvf bs=1 seek=120 conv=notrunc 2>/dev/null
cmp -s v120.sdvf tampered.sdvf && fail "the test package was not modified"
install_pkg tampered.sdvf 2>err.txt && fail "a tampered package was accepted"
grep -q InvalidSignature err.txt || fail "expected InvalidSignature, got: $(cat err.txt)"

echo "--- a downgrade is rejected"
sign 1.0.5 3 v120.sh v105.sdvf >/dev/null || fail "sign 1.0.5"
install_pkg v105.sdvf 2>err.txt && fail "a downgrade was accepted"
grep -q Downgrade err.txt || fail "expected Downgrade, got: $(cat err.txt)"
status | grep -q '^trial: none' || fail "rejected packages must not leave a trial behind"

echo "--- a failing health check rolls the update back"
install_pkg v120.sdvf || fail "install 1.2.0"
"$BIN/sdv-fota-launcher" "$W/state" /bin/false && fail "the launcher should report the failed trial"
status | grep -q '^active_version: 1.1.0' || fail "1.1.0 should still be active after the rollback"
status | grep -q '^last_trial_outcome: rolled_back' || fail "the trial should be recorded as rolled back"
echo "--- final state"; status
DRYRUN

fota_dry_run() {
  mkdir -p "$WORK/fota" || return 1
  if [ "$(id -u)" = 0 ]; then
    # root (a container): run it as `nobody`, like the launcher would run the application as an unprivileged user
    chmod 755 "$WORK" && chown 65534:65534 "$WORK/fota" && chmod 700 "$WORK/fota" || return 1
    setpriv --reuid=65534 --regid=65534 --clear-groups sh "$WORK/fota_dry_run.sh" "$FOTA_BIN" "$WORK/fota"
  else
    chmod 700 "$WORK/fota" && sh "$WORK/fota_dry_run.sh" "$FOTA_BIN" "$WORK/fota"
  fi
}
if [ -x "$FOTA_BIN/sdv-fota" ] && [ -x "$FOTA_BIN/sdv-fota-sign" ] && [ -x "$FOTA_BIN/sdv-fota-launcher" ]; then
  step "FOTA dry run" fota_dry_run
else
  record SKIP 0 "FOTA dry run" "the build did not produce the tools"
fi

# ---------------------------------------------------------------- 5. devices
note "== 5. I2C / GPIO devices (read-only listing; no pin is requested or changed)"
DISCOVER="$ROOT/rust/sdv-picarx/target/debug/sdv-picarx-discover"
if [ -x "$DISCOVER" ]; then
  step "sdv-picarx-discover" "$DISCOVER"
  sed 's/^/  /' "$LOG/sdv-picarx-discover.log" | head -40
else
  record SKIP 0 "sdv-picarx-discover" "not built"
fi

# ---------------------------------------------------------------- summary
printf '\n== summary  (%s, %s, %s)\n' "$MODEL" "$OS" "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo 'no git')"
printf '  %s\n' "${RESULTS[@]}"
printf 'total: %ss, logs: %s\n' "$SECONDS" "$LOG"
if [ "$FAILED" = 0 ]; then
  echo "RESULT: OK"
  rm -rf "$WORK/fota" "$WORK/fota_dry_run.sh" # keys and images of the dry run; the logs stay
else
  echo "RESULT: $FAILED step(s) FAILED - please attach this summary and the log files named above"
fi
exit $((FAILED > 0))
