# Pi 5 FOTA host workflow

This is the development workflow for the Linux application-image backend. It updates only files
under `/var/lib/sdv-fota`; it does not write the Pi OS, EEPROM, boot partition, or raw devices.
The host CLI can install from a local package file or stream a package from an HTTPS URL. HTTPS
uses the system CA store through `curl`, rejects HTTP URLs and URL-embedded credentials, and keeps
the Ed25519 package signature as the release authorization check. Deployments can additionally
require mutual TLS with an owner-only client private key; the client identity controls access to the
artifact endpoint but does not replace package signature verification. When mutual TLS is enabled,
the client caps redirects at zero so curl cannot forward the device identity to a redirect host; the
artifact endpoint must return the package directly.

## Build

On the Pi 5, build the Linux FOTA tools and PiCar-X vehicle image from the repository:

```sh
cargo build --release --manifest-path rust/sdv-fota/Cargo.toml --bin sdv-fota --bin sdv-fota-launcher
cargo build --release --manifest-path rust/sdv-picarx/Cargo.toml --bin sdv-picarx --bin sdv-picarx-health --bin sdv-picarx-discover --bin sdv-picarx-ecu-worker
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota /usr/bin/sdv-fota
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota-launcher /usr/bin/sdv-fota-launcher
sudo install -m 0755 rust/sdv-picarx/target/release/sdv-picarx-health /usr/bin/sdv-picarx-health
sudo install -m 0755 rust/sdv-picarx/target/release/sdv-picarx-discover /usr/bin/sdv-picarx-discover
sudo install -m 0755 rust/sdv-picarx/target/release/sdv-picarx-ecu-worker /usr/bin/sdv-picarx-ecu-worker
```

Before editing the PiCar-X config, run `sdv-picarx-discover` to list the actual I2C device paths,
GPIO chip labels, and GPIO line offsets on that Pi. It does not request or change any pins.

The signing utility is built separately on a protected release host with
`--features host-signing --bin sdv-fota-sign`. Do not build or provision a signing seed on the Pi.
Keep the seed in a regular file with Unix mode `0600`; the signer rejects group/other access and
zeroes both its decoded key and temporary file buffer. The `install-url` command requires `curl`
at runtime; local-file `install` does not.

## Provision the first image

Create the unprivileged application account. The launcher and installer keep the slot images and
boot metadata root-owned; candidate applications run as `sdv-app` and cannot replace the image or
rewrite boot state. Start with the disabled commissioning config, edit the device paths, and keep
forward power at zero until the motor and steering values are calibrated on the vehicle. Install
the initial PiCar-X image at `/var/lib/sdv-fota/slots/a/sdv-image.bin`, owned by root with mode
`0755`:

```sh
sudo useradd --system --user-group --home-dir /nonexistent --no-create-home --shell /usr/sbin/nologin sdv-app
sudo install -d -o root -g root -m 0755 /var/lib/sdv-fota/slots/a /var/lib/sdv-fota/slots/b
sudo install -d -o root -g root -m 0755 /etc/sdv-picarx
sudo install -o root -g root -m 0644 rust/sdv-picarx/config/pi5.fota.disabled.example.json /etc/sdv-picarx/vehicle.json
sudo install -o root -g root -m 0755 rust/sdv-picarx/target/release/sdv-picarx /var/lib/sdv-fota/slots/a/sdv-image.bin
sudo sdv-fota init --state-root /var/lib/sdv-fota --active-slot a \
  --active-version 1.0.0 --security-version 1
printf 'SDV_FOTA_APP_UID=%s\nSDV_FOTA_APP_GID=%s\nSDV_FOTA_ECU_APP_UID=%s\nSDV_FOTA_ECU_APP_GID=%s\nSDV_FOTA_ECU_APP_GROUPS=\nSDV_PICARX_CONFIG=/etc/sdv-picarx/vehicle.json\n' "$(id -u sdv-app)" "$(id -g sdv-app)" "$(id -u sdv-app)" "$(id -g sdv-app)" | sudo tee /etc/default/sdv-fota-launcher >/dev/null
sudo chown root:root /etc/default/sdv-fota-launcher
sudo chmod 0644 /etc/default/sdv-fota-launcher
```

The configured state root, `slots` directory, slot directories, image files, and boot records must
be owned by the account running the FOTA tools (root in this setup). Directories and files must not
be writable by group or other users. The tools never follow symlinked state roots, slot directories,
images, boot records, or lock files. Update images and boot records are created through unique, exclusive
temporary files before being atomically renamed into place.

The existing Pi host keeps Engine, Brake, and Steering in-process by default. To have the vehicle
coordinator spawn the worker processes, add this object to `/etc/sdv-picarx/vehicle.json`:

```json
"virtual_ecu_workers": {
  "binary": "/usr/bin/sdv-picarx-ecu-worker",
  "socket_directory": "/run/sdv-picarx/workers"
}
```

The coordinator starts all three workers and remains the sole owner of sensor input and GPIO/I2C
actuation. It sends requests to every worker before waiting on the shared coordinator socket. Every
response must match its worker path, ECU, and boot-monotonic cycle ID before the 10 ms cycle's
remaining budget expires. Missing workers, malformed, duplicate, or stale responses, or a deadline
miss stop the platform without applying partial or previous-cycle commands. The socket directory
is owner-only; worker and coordinator socket cleanup checks the recorded inode. The worker executable
must be a regular executable owned by root or the host account and cannot be group/other writable.
Install a root-owned executable for deployment; host-owned executables are accepted for local
testing. Spawned workers inherit the vehicle service's UID, supplementary groups, and capabilities,
so this mode is process separation only, not a privilege boundary.

For systemd-managed ECU worker processes, initialize a verified active image under each per-ECU
state root and install the external-worker sample config, which explicitly selects `mode: external`
and omits `binary`. The default mode is `spawn` for compatibility; a missing spawn binary is rejected
instead of silently changing modes:

```sh
sudo install -o root -g root -m 0644 rust/sdv-picarx/config/pi5.external-workers.disabled.example.json /etc/sdv-picarx/vehicle.json
sudo install -m 0644 deploy/systemd/sdv-fota-launcher.service /etc/systemd/system/
sudo install -m 0644 deploy/systemd/sdv-fota-ecu@.service /etc/systemd/system/
sudo install -m 0644 deploy/systemd/sdv-fota-workers.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl stop sdv-fota-workers.target
sudo systemctl restart sdv-fota-launcher.service
sudo systemctl enable --now sdv-fota-workers.target
```

With `mode` set to `external` and `binary` omitted, the host binds `coordinator.sock`, waits for all
three existing worker sockets, and neither starts nor stops worker processes or removes their socket paths. It owns only
the coordinator socket. The worker target starts the `engine`, `brake`, and `steering` service
instances after the host launcher; the units use fixed matching `--target`/`--ecu` values and the
FOTA launcher's active-only ECU API. They retry while the vehicle application starts and the
coordinator socket is unavailable. The coordinator does not begin its 10 ms task cycle until all
three worker sockets pass readiness checks. Each worker holds a mode-0600 per-socket lock, refuses
a second live instance, and reclaims only an unresponsive socket owned by the expected UID after
rechecking the socket inode. This also lets systemd restart a worker after SIGKILL leaves a stale
socket path behind. The host unit prepares the private worker directory before the app starts;
external-connect mode requires that existing directory and does not create it.

For this initial socket permission design, set `SDV_FOTA_ECU_APP_UID/GID` to the existing
`sdv-app` UID/GID and `SDV_FOTA_ECU_APP_GROUPS=` empty. ECU services therefore receive no `gpio` or
`i2c` supplementary groups. They share the vehicle process UID, so this is not OS privilege
isolation; a dedicated ECU account can be introduced later. The worker code does not open GPIO/I2C.
This PR does not implement ECU candidate/trial startup, output hold, candidate health, socket route
cutover, or coordinator-managed commit/rollback; those belong to the next update integration stage.
Omitting `virtual_ecu_workers` leaves the in-process runtime unchanged. Details and disabled-output
sample configs are in
[`rust/sdv-picarx/README.md`](../../rust/sdv-picarx/README.md).

To switch from external workers to coordinator-spawn or in-process mode, stop both the host and
worker target, disable the target, install the root-owned vehicle config for the selected mode, and
start the host launcher alone:

```sh
sudo systemctl stop sdv-fota-launcher.service sdv-fota-workers.target
sudo systemctl disable sdv-fota-workers.target
# Install a root-owned config with mode=spawn and a binary, or omit virtual_ecu_workers entirely.
sudo install -o root -g root -m 0644 /path/to/vehicle-config.json /etc/sdv-picarx/vehicle.json
sudo systemctl start sdv-fota-launcher.service
```

To return to external mode, stop the host and worker target before changing configuration; `start`
does not reload the EnvironmentFile or vehicle config while the host unit is already active. Confirm
each ECU active slot and integrity record is provisioned, install the `mode: external` config, start
the host service, then enable the worker target. The worker units follow host launcher restarts and
retry coordinator startup; stopping only the workers target while
the external host config is active leaves the host fail-closed and causes its systemd restart policy
to retry until all worker sockets return.

```sh
sudo systemctl stop sdv-fota-launcher.service sdv-fota-workers.target
sudo install -o root -g root -m 0644 rust/sdv-picarx/config/pi5.external-workers.disabled.example.json /etc/sdv-picarx/vehicle.json
sudo install -m 0644 deploy/systemd/sdv-fota-launcher.service /etc/systemd/system/
sudo systemctl start sdv-fota-launcher.service
sudo systemctl enable --now sdv-fota-workers.target
```

When upgrading a Pi provisioned by an earlier version without image digests, first replace the
`sdv-fota` binary while the existing launcher is still running. Verify the active slot image against
the approved release, then record its digest before installing/restarting the new launcher:

```sh
# Read active_slot with `sudo sdv-fota status --state-root /var/lib/sdv-fota`;
# use its lowercase value (`a` or `b`) below.
sudo sdv-fota record-active-integrity --state-root /var/lib/sdv-fota --slot a
```

This migration command hashes the current active executable; it does not establish the image's
release provenance. Do not run it until the active image has been checked against a trusted build
or release record. It refuses to run when the selected slot is not active or a trial is pending.

The systemd launcher retains only `CAP_SETUID` and `CAP_SETGID` so it can drop privileges before
starting either the application or its health command. It is otherwise restricted to writing
under `/var/lib/sdv-fota`.

For PiCar-X, the `vehicle` process may need access to Robot HAT devices. Add only the device
groups present on the target OS to the root-owned environment file. Resolve numeric IDs on the Pi
instead of assuming fixed IDs:

```sh
getent group i2c
getent group gpio
# Set SDV_FOTA_APP_GROUPS to the applicable numeric GIDs, comma-separated.
```

When both groups exist, add `SDV_FOTA_APP_GROUPS=<i2c-gid>,<gpio-gid>` to
`/etc/default/sdv-fota-launcher`. The launcher applies these supplementary groups to both the
vehicle process and its health check, while both run as the unprivileged `sdv-app` user.

Do not add hardware device groups to `SDV_FOTA_ECU_APP_GROUPS`. If ECU images need OS permissions
separate from the vehicle, configure a dedicated non-root ECU account. Reusing the vehicle UID does
not provide privilege isolation.

Provision the matching Ed25519 public key through the ECU's trusted configuration. The FOTA CLI
loads trusted keys from `/etc/sdv-fota/trusted-keys`. Provision the file as root, readable by
the `sdv` account, and not writable by group or other users. The file contains one key per line:

```text
# key-id followed by the 32-byte Ed25519 public key in lowercase or uppercase hex
release-2026 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

For example, use the public key printed by `sdv-fota-sign` on the protected release host:

```sh
sudo install -d -o root -g root -m 0755 /etc/sdv-fota
PUBLIC_KEY_HEX='<actual-public-key-hex>'
printf '%s %s\n' 'release-2026' "$PUBLIC_KEY_HEX" | sudo tee /etc/sdv-fota/trusted-keys >/dev/null
sudo chown root:root /etc/sdv-fota/trusted-keys
sudo chmod 0644 /etc/sdv-fota/trusted-keys
```

The installer rejects a trust file that is not a regular root-owned file or is writable by group
or other users. Keep signing seeds on the release host; only public keys belong on the Pi.

## Sign and install an update

On the release host, sign an application image. The tool prints the key ID and public key so the
release process can verify which trust key is needed on the target:

```sh
chmod 0600 /secure/release.seed
sdv-fota-sign --seed /secure/release.seed --key-id release-2026 --target vehicle \
  --version 1.1.0 --security-version 2 --trial-ms 5000 --config-schema 1 \
  --signal-if-version 1 --image sdv-image.bin --output update.sdvf
```

Deliver `update.sdvf` through the deployment's chosen transport. On the Pi, install it using the
provisioned public key and the active ECU compatibility values:

```sh
# Install a local package or stream one from an HTTPS artifact endpoint:
sudo sdv-fota install --state-root /var/lib/sdv-fota --package update.sdvf \
  --target vehicle --config-schema 1 --signal-if-version 1
sudo sdv-fota install-url --state-root /var/lib/sdv-fota \
  --url https://updates.example/vehicle/update.sdvf --target vehicle \
  --config-schema 1 --signal-if-version 1
# When the endpoint requires mutual TLS, provision a root-owned certificate and private key:
sudo install -d -o root -g root -m 0750 /etc/sdv-fota/tls
sudo install -o root -g root -m 0644 device.crt /etc/sdv-fota/tls/device.crt
sudo install -o root -g root -m 0600 device.key /etc/sdv-fota/tls/device.key
sudo sdv-fota install-url --state-root /var/lib/sdv-fota \
  --url https://updates.example/vehicle/update.sdvf --target vehicle \
  --config-schema 1 --signal-if-version 1 \
  --client-cert /etc/sdv-fota/tls/device.crt --client-key /etc/sdv-fota/tls/device.key
sudo sdv-fota status --state-root /var/lib/sdv-fota
sudo sdv-fota verify-images --state-root /var/lib/sdv-fota
sudo systemctl restart sdv-fota-launcher.service
```

### Stage an individual virtual ECU image

The Linux CLI can keep A/B slots, boot metadata, active version, security counter, and pending
trial metadata separate for `engine`, `brake`, and `steering`. The `vehicle` target continues to
use `/var/lib/sdv-fota` directly; each virtual ECU uses a fixed subdirectory under the same base,
such as `/var/lib/sdv-fota/ecus/engine`. Target names are parsed as an enum and are not used as
arbitrary path components. Initialize each ECU's existing active image and metadata separately
before staging packages for it. For example:

```sh
# Provision the current Engine image and its independent A/B state.
sudo install -d -o root -g root -m 0755 /var/lib/sdv-fota/ecus/engine/slots/a \
  /var/lib/sdv-fota/ecus/engine/slots/b
sudo install -o root -g root -m 0755 engine-current.bin \
  /var/lib/sdv-fota/ecus/engine/slots/a/sdv-image.bin
sudo sdv-fota init --state-root /var/lib/sdv-fota --target engine \
  --active-slot a --active-version 1.0.0 --security-version 1

# The signed manifest target must match the CLI target.
sdv-fota-sign --seed /secure/release.seed --key-id release-2026 --target engine \
  --version 1.1.0 --security-version 2 --trial-ms 5000 --config-schema 1 \
  --signal-if-version 1 --image engine-update.bin --output engine-update.sdvf
sudo sdv-fota install --state-root /var/lib/sdv-fota --package engine-update.sdvf \
  --target engine --config-schema 1 --signal-if-version 1
sudo sdv-fota status --state-root /var/lib/sdv-fota --target engine
sudo sdv-fota verify-images --state-root /var/lib/sdv-fota --target engine
```

Use `--target brake` or `--target steering` for those state roots. `init`, `install`/`install-url`,
`status`, `verify-images`, `record-active-integrity`, and `rollback-trial` all select the same
target-specific root. Omitting `--target` from the state-management commands retains the legacy
`vehicle` root. The signed package is verified and written to that ECU's inactive slot, and its
version, security counter, and trial record stay independent from other targets.

This prepares per-ECU staging and trial metadata. The installed systemd unit still starts the whole
vehicle application from the `vehicle` state root. An explicit target-aware ECU invocation can
start only the verified active image; it does not start a pending candidate or update its trial
metadata. Do not restart the vehicle unit expecting an ECU-target package to run. Coordinator
worker trial lifecycle and its health/commit connection are a follow-on integration.

`status` is a diagnostic report and exits successfully even when it reports an image mismatch.
`verify-images` checks the active image and any pending trial image, prints the same integrity
states, and exits nonzero unless every bootable image has a verified digest. It can be used as a
deployment gate or a monitoring check.

The launcher sets `SDV_TRIAL_SLOT` on a trial candidate. The PiCar-X image recognizes this mode and
holds all vehicle actuator commands at zero while its health is checked. Health reports ready only
when the latest fresh vehicle snapshot estimates speed within
`trial_stationary_speed_tolerance_mps` and throttle input is within
`trial_stationary_throttle_tolerance`, in addition to a recent successful actuator write. Otherwise
the trial times out or fails and rolls back. Configure these limits in the vehicle JSON before
deployment. The gate depends on the external estimator/input peer and cannot independently prove
physical standstill; mechanically secure the vehicle during a trial. After a successful commit, the
launcher stops the safe-mode trial process. The systemd unit restarts it from the committed slot in
normal mode. A caller running the launcher directly must restart it after a successful trial.

If a trial is pending but the launcher cannot complete its automatic recovery, an operator can
clear the trial metadata explicitly. This command keeps the current active slot and makes it the
next slot the launcher starts; it does not stop a process that is already running. Stop the vehicle
first, clear the trial, then restart the launcher to run the previous active image:

```sh
sudo sdv-fota rollback-trial --state-root /var/lib/sdv-fota
sudo systemctl restart sdv-fota-launcher.service
```

The `init` command refuses to record a slot without a non-empty regular executable image and
records its SHA-256 digest. The installer records the digest of each staged image after writing it;
the existing package readback check must also match the signed manifest before a trial is
scheduled. On restart, the launcher checks the selected image against its recorded digest before
starting it, then repeatedly runs the configured health command until it succeeds or `trial_ms`
expires. A trial image that is missing, not executable, or has a mismatched digest is rolled back.
After the first healthy result, the launcher requires the process to remain alive and the health
check to keep passing for up to two seconds (half the trial window when that is shorter) before it
commits. An exit or failed health check during this stability window rolls the trial back.
During a trial, the health command
receives `SDV_TRIAL_SLOT` and `SDV_IMAGE_PATH`; it must exit promptly and return success only when
the application is healthy. A failed or expired trial is rolled back. A successful trial commits
the slot and security version before the launcher waits for the application process.
`sdv-fota status` reports `active_image_integrity` and `trial_image_integrity` as `verified`,
`missing`, `mismatch_or_unbootable`, `unsafe_path`, or `unavailable` so operators can diagnose
why a slot was rejected. It also retains the most recent trial's outcome, slot, version, and
security version across restarts (`committed` or `rolled_back`) for post-update diagnosis.

The launcher keeps its existing positional form and adds a target-aware form with fixed application
arguments. Use the legacy positional form only with the vehicle base state root. It retains the
existing trial, health, commit, and rollback behavior; do not pass a path such as
`/var/lib/sdv-fota/ecus/engine` to it, because positional mode has no ECU active-only guard. Use the
target-aware ECU form below instead. The legacy positional form and `vehicle` target use the vehicle
health program:

```text
sdv-fota-launcher --state-root <base-dir> --target vehicle \
  --health-program <configured-vehicle-health-check> [--health-arg <arg> ...] [--app-arg <arg> ...]
```

`vehicle` uses the base state directory; each virtual ECU resolves to its fixed `ecus/<target>`
directory. Each repeated `--app-arg` is passed as one argument to the digest-verified slot image
with `Command::args`; values such as spaces, semicolons, and `$()` are not shell syntax. Keep the
launcher invocation and its arguments in root-owned service configuration; arguments are not read
from a package manifest. The root launcher owns vehicle launch-time trial selection, commit, and
rollback. Package installation and operator-triggered rollback remain explicit administrator
operations through `sdv-fota`.

ECU targets have a separate active-only service form and do not take a health command:

```text
sdv-fota-launcher --state-root <base-dir> --target <engine|brake|steering> \
  --app-arg --ecu --app-arg <matching-target> [--app-arg <worker-argument> ...]
```

An ECU launch requires exactly one matching `--ecu <target>` pair. The launcher verifies and runs
only that ECU's recorded active slot. It does not call `select_for_boot`, decrement `boots_left`, or
change pending trial metadata; it removes `SDV_TRIAL_SLOT` and `SDV_IMAGE_PATH` from the child
environment and performs no health check, trial commit, or rollback. A pending candidate stays
pending for a later coordinator-managed trial flow. The legacy positional and `vehicle` target
trial semantics are unchanged.

ECU launches use
`SDV_FOTA_ECU_APP_UID` and `SDV_FOTA_ECU_APP_GID`, plus optional
`SDV_FOTA_ECU_APP_GROUPS`; missing groups means an empty supplementary-group list, and the
launcher clears inherited groups before exec. Vehicle launches keep using `SDV_FOTA_APP_*`, so
vehicle `i2c`/`gpio` groups are not passed to an ECU image. The ECU UID/GID should identify a
dedicated non-root account. If it is configured to the same UID as the vehicle process, this does
not provide privilege isolation. The launcher requires `CAP_SETGID` to clear inherited groups.
The fixed `ecus` parent must be a real launcher-owned directory; a symlink is rejected. The example
`sdv-picarx-health` command is vehicle-only and cannot validate an ECU worker. This active-only ECU
launch is not connected to the Pi coordinator's output hold, candidate health on trial sockets,
route cutover, or post-cutover commit/rollback, and must not be treated as that update procedure.

Install the example unit from `deploy/systemd/sdv-fota-launcher.service`. Its health command checks
the vehicle runtime's Unix socket, which reports ready only after a successful control cycle. The
vehicle image waits for a peer to send fresh driver and state-estimate packets to
`/run/sdv-picarx/input.sock`; that peer must run as `sdv-app` or have permission to write to the
socket. Each packet carries its monotonic sensor-sample timestamp so queued old state is rejected;
the protocol and limits are documented in `rust/sdv-picarx/README.md`:

```sh
sudo install -m 0644 deploy/systemd/sdv-fota-launcher.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sdv-fota-launcher.service
```

`sdv-picarx-health` reports ready only after a recent ECU cycle and zeroed actuator write with an
estimated stationary vehicle and zero throttle demand. Keep the external input and vehicle-state
process available during the trial; the FOTA trial fails when snapshots are stale, missing, or
report motion.

## Current boundary

The PiCar-X deployment uses the complete Rust Linux runtime as one `vehicle` FOTA image.
`sdv-core::runtime::VehicleRuntime` runs VCU, Engine, Brake, and Steering as separate virtual ECU
tasks with their own RTE state and the same virtual bus contract as the Rust simulator; this does not
require separate physical CPUs. The `vehicle` target remains a single A/B application image, while
the Linux CLI can also stage individually signed `engine`, `brake`, or `steering` packages into
separate fixed state roots and record independent trial metadata. The installed systemd unit still
uses the `vehicle` root; an explicit ECU-targeted launcher invocation starts only the verified
active image and leaves trial metadata unchanged. The legacy positional launcher is vehicle-root
only; passing an `ecus/<target>` directory to it preserves trial semantics and can consume or boot
a pending candidate, so use the target-aware ECU form instead. An ECU target requires a matching
`--app-arg --ecu --app-arg <target>` pair and a dedicated non-root identity from
`SDV_FOTA_ECU_APP_UID/GID`; supplementary ECU groups default to empty. The installed
`sdv-picarx-health` command checks vehicle runtime health and is not an ECU worker health check, so
the example systemd unit remains vehicle-only. Coordinator-managed trial output hold, candidate
health, route cutover, and post-cutover commit/rollback remain the next integration stage. Virtual ECU worker processes can
be enabled as an alternative Linux runtime mode, but coordinator-managed worker trial startup,
output hold, trial-socket health checks, atomic route cutover, and commit/rollback after cutover are
not connected. The Pi path can sign a package on a separate host, verify and
stage it incrementally, and exercise whole-vehicle A/B trial boot and rollback on Linux. HTTPS
artifact transfer uses the device's system CA store and can use mutual TLS. Private repository
credentials, remote campaign orchestration, a real vehicle-state estimator/input peer, actual Pi 5
hardware validation, and calibrated PiCar-X output remain deployment work.
AUTOSAR Classic integration additionally requires the chosen
DCM/Csm/NvM/MemIf/Fls/Bootloader bindings described in
[the Classic architecture proposal](fota_classic_architecture.md).
