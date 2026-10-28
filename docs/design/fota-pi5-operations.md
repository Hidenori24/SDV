# Pi 5 FOTA host workflow

This is the development workflow for the Linux application-image backend. It updates only files
under `/var/lib/sdv-fota`; it does not write the Pi OS, EEPROM, boot partition, or raw devices.
The host CLI can install from a local package file or stream a package from an HTTPS URL. HTTPS
uses the system CA store through `curl`, rejects HTTP URLs and HTTP redirects, and keeps the
Ed25519 package signature as the release authorization check.

## Build

On the Pi 5, build the Linux FOTA tools and PiCar-X vehicle image from the repository:

```sh
cargo build --release --manifest-path rust/sdv-fota/Cargo.toml --bin sdv-fota --bin sdv-fota-launcher
cargo build --release --manifest-path rust/sdv-picarx/Cargo.toml --bin sdv-picarx --bin sdv-picarx-health --bin sdv-picarx-discover
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota /usr/bin/sdv-fota
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota-launcher /usr/bin/sdv-fota-launcher
sudo install -m 0755 rust/sdv-picarx/target/release/sdv-picarx-health /usr/bin/sdv-picarx-health
sudo install -m 0755 rust/sdv-picarx/target/release/sdv-picarx-discover /usr/bin/sdv-picarx-discover
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
printf 'SDV_FOTA_APP_UID=%s\nSDV_FOTA_APP_GID=%s\nSDV_PICARX_CONFIG=/etc/sdv-picarx/vehicle.json\n' "$(id -u sdv-app)" "$(id -g sdv-app)" | sudo tee /etc/default/sdv-fota-launcher >/dev/null
sudo chown root:root /etc/default/sdv-fota-launcher
sudo chmod 0644 /etc/default/sdv-fota-launcher
```

The configured state root, `slots` directory, slot directories, image files, and boot records must
be owned by the account running the FOTA tools (root in this setup). Directories and files must not
be writable by group or other users. The tools never follow symlinked state roots, slot directories,
images, boot records, or lock files. Update images and boot records are created through unique, exclusive
temporary files before being atomically renamed into place.

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
sudo sdv-fota status --state-root /var/lib/sdv-fota
sudo sdv-fota verify-images --state-root /var/lib/sdv-fota
sudo systemctl restart sdv-fota-launcher.service
```

`status` is a diagnostic report and exits successfully even when it reports an image mismatch.
`verify-images` checks the active image and any pending trial image, prints the same integrity
states, and exits nonzero unless every bootable image has a verified digest. It can be used as a
deployment gate or a monitoring check.

The Pi Linux trial health check does not currently read a vehicle-stopped interlock. On a real
PiCar-X, restart the launcher only after the vehicle is fully stopped and the external driver-input
peer is no longer requesting motion. The health check proves a recent control cycle and stable
process; it does not prove that the vehicle is stationary.

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
why a slot was rejected.

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

`sdv-picarx-health` reports ready only after the vehicle application completes a recent ECU cycle
and actuator write. Keep the external input and vehicle-state process available during the trial;
the FOTA trial fails when it cannot provide fresh snapshots.

## Current boundary

The PiCar-X deployment uses the complete Rust Linux runtime as one `vehicle` FOTA image.
`sdv-core::runtime::VehicleRuntime` runs VCU, Engine, Brake, and Steering as separate virtual ECU
tasks with their own RTE state and the same virtual bus contract as the Rust simulator; this does not
require separate physical CPUs. The Pi Linux installer accepts the `vehicle` target because
these tasks share one A/B application image; `engine`, `brake`, and `steering` cannot be installed
as independent Pi images yet. The Pi path can sign a
package on a separate host, verify and stage it incrementally, and exercise
A/B trial boot and rollback on Linux. HTTPS artifact transfer uses the device's system CA store;
mutual TLS, private repository credentials, remote campaign orchestration, a real vehicle-state
estimator/input peer, actual Pi 5 hardware validation, and calibrated PiCar-X output remain
deployment work.
AUTOSAR Classic integration additionally requires the chosen
DCM/Csm/NvM/MemIf/Fls/Bootloader bindings described in
[the Classic architecture proposal](fota_classic_architecture.md).
