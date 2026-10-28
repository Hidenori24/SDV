# Pi 5 FOTA host workflow

This is the development workflow for the Linux application-image backend. It updates only files
under `/var/lib/sdv-fota`; it does not write the Pi OS, EEPROM, boot partition, or raw devices.
The host CLI can install from a local package file or stream a package from an HTTPS URL. HTTPS
uses the system CA store through `curl`, rejects HTTP URLs and HTTP redirects, and keeps the
Ed25519 package signature as the release authorization check.

## Build

On the Pi 5, build the Linux host commands from the repository:

```sh
cargo build --release --manifest-path rust/sdv-fota/Cargo.toml --bin sdv-fota --bin sdv-fota-launcher
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota /usr/bin/sdv-fota
sudo install -m 0755 rust/sdv-fota/target/release/sdv-fota-launcher /usr/bin/sdv-fota-launcher
```

The signing utility is built separately on a protected release host with
`--features host-signing --bin sdv-fota-sign`. Do not build or provision a signing seed on the Pi.
The `install-url` command requires `curl` at runtime; local-file `install` does not.

## Provision the first image

Create the service account and data directories, then put the trusted initial executable at
`/var/lib/sdv-fota/slots/a/sdv-image.bin`. Set its owner to `sdv:sdv` and mode to `0755`. Initialize
boot metadata with the initial image's version and security version:

```sh
sudo useradd --system --home-dir /var/lib/sdv-fota --no-create-home sdv
sudo install -d -o sdv -g sdv -m 0750 /var/lib/sdv-fota/slots/a /var/lib/sdv-fota/slots/b
sudo install -o sdv -g sdv -m 0755 initial-sdv-app /var/lib/sdv-fota/slots/a/sdv-image.bin
sudo -u sdv sdv-fota init /var/lib/sdv-fota a 1.0.0 1
```

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
sdv-fota-sign /secure/release.seed release-2026 engine 1.1.0 2 5000 1 1 sdv-app update.sdvf
```

Deliver `update.sdvf` through the deployment's chosen transport. On the Pi, install it using the
provisioned public key and the active ECU compatibility values:

```sh
# Install a local package or stream one from an HTTPS artifact endpoint:
sudo -u sdv sdv-fota install /var/lib/sdv-fota update.sdvf engine 1 1
sudo -u sdv sdv-fota install-url /var/lib/sdv-fota https://updates.example/engine/update.sdvf engine 1 1
sdv-fota status /var/lib/sdv-fota
sudo systemctl restart sdv-fota-launcher.service
```

The restart selects the pending trial slot. The launcher starts the image and repeatedly runs the
configured health command until it succeeds or `trial_ms` expires. During a trial, that command
receives `SDV_TRIAL_SLOT` and `SDV_IMAGE_PATH`; it must exit promptly and return success only when
the application is healthy. A failed or expired trial is rolled back. A successful trial commits
the slot and security version before the launcher waits for the application process.

Install the example unit from `deploy/systemd/sdv-fota-launcher.service` after providing the
`sdv-health-check` program. The health check is application-specific; the repository does not
assume a particular SDV process readiness protocol.

## Current boundary

The Pi path can sign a package on a separate host, verify and stage it incrementally, and exercise
A/B trial boot and rollback on Linux. HTTPS artifact transfer uses the device's system CA store;
mutual TLS, private repository credentials, remote campaign orchestration, actual Pi 5 hardware
validation, and a production health-check implementation still belong to deployment integration.
AUTOSAR Classic integration additionally requires the chosen
DCM/Csm/NvM/MemIf/Fls/Bootloader bindings described in
[the Classic architecture proposal](fota_classic_architecture.md).
