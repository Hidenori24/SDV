# PiCar-X platform adapter

This crate maps SDV virtual ECU commands to a replaceable `RobotHat` interface. Supply a fresh
`InputSource`, a hardware implementation, and calibration values measured for the car. The
adapter limits forward power and steering travel; emergency stop and any positive brake request
stop the drive motors. Cutting motor power does not guarantee a particular deceleration.

The mapping is open loop: requested acceleration is scaled to normalized forward power. It does
not estimate speed, implement a mechanical brake, or calibrate the car. In particular, cutting
motor power does not guarantee a particular deceleration.

`pwm` provides the Robot HAT MCU PWM register framing behind a `RegisterTransport` interface.
Channels share timers, so a hardware adapter must coordinate their frequency and period. A failed
timer configuration can leave a partial write; the caller must stop the motors and reinitialize
before proceeding. The register mapping was checked against [SunFounder's Robot HAT repository](https://github.com/sunfounder/robot-hat).

`robothat::Actuators` binds the Robot HAT PWM registers to steering channel P2 and rear motor
channels P13/P12. It requires explicit motor polarity and duty calibration. It configures shared
motor and servo timers and stops both motor PWM channels during initialization. Direction pins are
provided through the `MotorDirections` port so the GPIO backend can be replaced independently.

On Linux, `linux_gpio::LinuxMotorDirections` requests both direction lines as inactive outputs in
one GPIO character-device request. The chip device path and both line offsets must be supplied
explicitly for the target Pi; it does not assume that the same `/dev/gpiochipN` exists on every
image.

On Linux, `linux_i2c::I2cTransport` opens an explicitly configured i2c-dev path and MCU address,
then sends each PWM register message as one I2C transaction. The current crate still needs a fresh
`linux_input::UnixDatagramInputSource` receives driver input, E-stop, and vehicle state estimates
over a local Unix datagram socket. It accepts only increasing sequence numbers and source timestamps
from the host's shared `CLOCK_MONOTONIC` clock. Packets must be at most 2048 bytes, and at most eight
queued frames are processed per ECU cycle. Missing, stale, future-dated, oversized, or excessive
input returns an error; `VehicleExecutor` then requests a motor stop. The sender should set
`sampled_at_monotonic_ns` with `time.monotonic_ns()` when it samples its sensors. Vehicle state must
come from a sensor or estimator process. This one-shot commissioning probe requests no drive and
asserts E-stop; it illustrates the packet fields, but it does not provide live telemetry:

```python
import json
import socket
import time

frame = {
    "sequence": 1,
    "sampled_at_monotonic_ns": time.monotonic_ns(),
    "throttle": 0.0,
    "brake": 0.0,
    "steer": 0.0,
    "estop": True,
    "vehicle": {"t": 0.0, "x": 0.0, "y": 0.0, "yaw": 0.0,
                "v": 0.0, "yaw_rate": 0.0, "wheel_omega": 0.0},
}
with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as peer:
    peer.sendto(json.dumps(frame).encode(), "/run/sdv-picarx/input.sock")
```

A real input peer must publish continuously, increment `sequence`, and timestamp each actual sensor
sample. The receiver treats the timestamp as the age of the measurement, not the packet arrival.

## Linux virtual ECU worker IPC

`virtual_ecu_worker::VirtualEcuWorker` owns one Engine, Brake, or Steering `VirtualEcuTask` and
handles one request per 10 ms cycle over a connected `UnixDatagram`. Version 1 requests carry the
ECU id, a strictly increasing cycle id, and zero to two encoded bus frames; responses echo the
cycle id and contain that ECU's command and alive frame. The adapter rejects oversized or malformed
packets, wrong ECU/frame routing, and repeated or reversed cycle ids before advancing the task.
`serve_once` does not create socket paths or schedule cycles. On Pi/Linux, an optional
`virtual_ecu_workers` host setting starts one worker process for each ECU. The default `spawn` mode
requires a worker `binary` and keeps the existing coordinator-managed process behavior. The
`external` mode must be explicit and omits `binary`; it connects to worker processes managed outside
the vehicle process. In both cases the vehicle coordinator remains the sole owner of GPIO, I2C,
sensor input, and actuator output. CRC32 detects accidental packet corruption but does not
authenticate a peer, so the coordinator validates socket ownership, mode, peer path, ECU id, and
cycle id.

The Linux `sdv-picarx-ecu-worker` executable binds one worker socket, connects to an already
running coordinator socket, then handles requests until SIGINT or SIGTERM. It accepts only absolute
socket paths in owner-controlled directories, requires the coordinator socket to be owner-only and
owned by the same account, and creates the worker socket with mode `0600`. If the worker path
already exists (including a socket or symlink), startup refuses to replace it. On shutdown it
removes only the socket inode created by that process. The coordinator must send strictly
increasing cycle ids and the input frames for each 10 ms step:

```sh
cargo build --release --manifest-path rust/sdv-picarx/Cargo.toml --bin sdv-picarx-ecu-worker
./rust/sdv-picarx/target/release/sdv-picarx-ecu-worker --ecu engine \
  --worker-socket /run/sdv-picarx/workers/engine.sock \
  --coordinator-socket /run/sdv-picarx/coordinator.sock
```

Use `brake` or `steering` for the other worker processes. This executable runs the ECU software
only; it does not open GPIO/I2C devices or own vehicle-level coordination.

## Linux vehicle host

The `sdv-picarx` executable composes the I2C, GPIO, input, PiCar-X, and virtual ECU runtime ports.
It accepts one named option, `--config <file.json>`. The JSON config contains:

| Field | Meaning |
| --- | --- |
| `i2c_device`, `robot_hat_address` | Linux I2C device path and 7-bit Robot HAT address |
| `gpio_chip`, `left_direction_line`, `right_direction_line` | GPIO chip device path and line offsets |
| `input_socket`, `input_max_age_ms` | Local input socket path and maximum input age |
| `trial_stationary_speed_tolerance_mps`, `trial_stationary_throttle_tolerance` | Maximum estimated speed and requested throttle accepted by the FOTA trial health check |
| `virtual_ecu_workers` | Optional worker-process mode. `mode` defaults to `spawn` for existing configs and requires a trusted `binary`; set `mode` to `external` and omit `binary` to connect to externally managed workers. External mode may set `candidate_socket_directory` for isolated shadow workers. Invalid mode/binary combinations are rejected. Omit the whole field to keep in-process ECU tasks. |
| `motor` | Forward direction polarity and measured minimum/maximum PWM duty |
| `vehicle` | Measured drive and steering calibration used by the ECU adapter |

The host validates calibration before opening devices, initializes the motor PWM to zero, and waits
for a first valid input packet before starting the 10 ms ECU loop. Missing or stale input, hardware
I/O failure, a missed ECU deadline, SIGINT, and SIGTERM request a motor stop. The process removes
its input socket path on exit. Vehicle state in each packet must be supplied by a real sensor or
state estimator; this crate does not provide one. A health socket reports ready only after a
successful actuator cycle, while the input remains fresh, the estimated speed stays within the
configured stationary tolerance, and throttle demand stays within its configured tolerance. When
`SDV_TRIAL_SLOT` is set to `a` or `b`, the runtime suppresses all actuator commands until the FOTA
launcher commits the image and restarts the service in normal mode. Missing, stale, or moving
snapshots fail the FOTA trial health check. This software interlock trusts the external state
estimator; it cannot independently prove physical standstill. The vehicle launcher also sets
`SDV_FOTA_PENDING_ECU_TRIAL=1` when any per-ECU boot record has a pending trial. The runtime then
keeps zero output held across host restarts until an administrator rolls back that trial and restarts
the vehicle service. The root FOTA launcher owns `/run/sdv-fota/control.sock` and passes its listener
descriptor to the vehicle process; the application UID cannot replace the socket with a fake server.
A root-only control request receives an acknowledgement only after the physical platform
successfully applies a zero-drive, centered-steering command. The
`sdv-picarx-health --socket <path>` command is suitable for the FOTA launcher. With no command line
arguments, the vehicle executable reads its config path from `SDV_PICARX_CONFIG`, as used by the
FOTA systemd service.

To opt into worker processes, add this object to the host JSON:

```json
"virtual_ecu_workers": {
  "binary": "/usr/bin/sdv-picarx-ecu-worker",
  "socket_directory": "/run/sdv-picarx/workers"
}
```

The coordinator starts Engine, Brake, and Steering workers and sends all three cycle requests
before waiting for responses. It accepts only one matching response from each worker before the
remaining 10 ms host-cycle budget expires. Missing workers, invalid packets, stale/duplicate
responses, or a missed deadline stop the platform; partial or previous-cycle commands are not
applied. The `socket_directory` is owner-only and owned by the host account; sockets are owner-only
and validated against their recorded inodes. In spawn mode, the worker binary must be a regular
executable owned by root or the host account, with no group/other write permission. Install a
root-owned binary in production; host-owned binaries are accepted for local testing. Spawned workers
inherit the host process UID, supplementary groups, and capabilities, so this process mode is not a
privilege boundary even though workers do not open GPIO/I2C. A future root-managed per-ECU service
boundary is needed for privilege separation. Worker processes receive a parent-death signal so they
exit if the host is killed.

To connect to separately launched workers, use
`config/pi5.external-workers.disabled.example.json`; it explicitly sets `mode` to `external` and
omits `binary`. Omitting `mode` defaults to `spawn`, so a missing spawn `binary` is rejected instead
of silently switching modes. The coordinator creates only `coordinator.sock`, waits for all three pre-existing worker
sockets, and never starts, stops, or removes those worker processes or socket paths. The supplied
systemd template starts each ECU through the FOTA launcher's active-only target form. During this
first integration, ECU services use the existing `sdv-app` UID/GID with empty supplementary groups;
they receive no `gpio` or `i2c` groups. Sharing the vehicle UID does not create OS privilege
isolation. Each worker holds a mode-0600 per-socket lock, refuses a second live instance, and only
reclaims an owner-owned stale socket in the private directory after the socket is unresponsive and
its inode is rechecked. The host systemd unit prepares the private worker and candidate directories;
external mode requires those directories to exist. When external mode sets
`candidate_socket_directory` to `/run/sdv-picarx/candidates`, the host binds a second coordinator
socket there. The root FOTA launcher separately owns `/run/sdv-fota/control.sock` in a root-owned
runtime directory and passes its already-open listener to the vehicle process. Start an ECU's
`sdv-fota-ecu-candidate@<ecu>.service` only after that ECU has a pending trial image and the vehicle
is stationary and physically secured. Candidate requests and responses use the separate socket and are processed
independently of active worker replies. Candidate workers are configured with both their shadow
coordinator and the active coordinator as approved peers; they validate each coordinator socket's
owner, mode, and inode, then reply to the socket that sent the request. This lets a promoted worker
answer through the active response socket without restarting its process. After validating active replies, the host enqueues a fixed
request snapshot through a capacity-one `try_send` queue. Candidate filesystem checks, datagram
I/O, timeout accounting, and journal writes run on a separate thread. A full queue or expired
snapshot skips only that shadow cycle, and candidate coordinator setup errors do not prevent active
worker startup. Status begins at `WarmingUp (0/100)` and counts matching shadow responses for
consecutive host-cycle sequence numbers with increasing wire cycle IDs; only the 100th consecutive
response changes it to `Ready`. A host-cycle gap, full or expired snapshot, timeout,
protocol/ECU/cycle mismatch, or socket inode failure resets the window. Queue-full invalidation is
visible immediately, before the shadow thread processes the next snapshot. Candidate command and
alive frames are discarded while shadowing; the active route remains in use. Candidate faults do
not fail or delay the active vehicle cycle. `Ready` is protocol-level shadow stability only, not
application-semantic or vehicle health.
Before the
candidate executable starts, its root launcher asks the host to latch the output hold and waits for
an acknowledgement after a successful zero-output apply. The hold keeps applying zero drive and
centered steering on every cycle. Candidate failures do not release the hold; no release API exists
in this slice. The coordinator still fails closed if an active worker is missing or returns an
invalid response.

Install and enable the worker units after initializing an active image in each ECU state root and
installing the target-aware FOTA launcher configuration:

```sh
sudo install -m 0644 deploy/systemd/sdv-fota-ecu@.service /etc/systemd/system/
sudo install -m 0644 deploy/systemd/sdv-fota-ecu-candidate@.service /etc/systemd/system/
sudo install -m 0644 deploy/systemd/sdv-fota-workers.target /etc/systemd/system/
sudo install -m 0644 deploy/systemd/sdv-fota-launcher.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl stop sdv-fota-workers.target
sudo systemctl restart sdv-fota-launcher.service
sudo systemctl enable --now sdv-fota-workers.target
```

After staging and verifying one ECU's pending trial package, secure the stationary vehicle so it
cannot move; zero motor power is not a mechanical brake. Confirm that the vehicle health command
reports `READY`, then start only that ECU's shadow worker while readiness remains current. For
example, with a valid Engine trial and the external config above:

```sh
sudo sdv-fota status --state-root /var/lib/sdv-fota --target engine
sudo sdv-fota verify-images --state-root /var/lib/sdv-fota --target engine
sudo sdv-picarx-health --socket /run/sdv-picarx/health.sock
sudo systemctl start sdv-fota-ecu-candidate@engine.service
sudo journalctl -u sdv-fota-launcher.service -f
```

The candidate launcher snapshots the pending slot and verifies its digest, then requires a
root-credentialed hold request to be acknowledged by the vehicle process after zero output reaches
the PiCar-X adapter. Only after that acknowledgement does it start the candidate. It does not call
`select_for_boot`; `boots_left`, active slot, trial metadata, and digests remain unchanged. Once the
candidate reports `Ready`, a root administrator can request an explicit route promotion for that
ECU:

```sh
sudo sdv-fota promote-candidate --control-socket /run/sdv-fota/control.sock --target engine
```

The inherited control endpoint accepts root peers only. The request confirms the sticky zero-output
hold, stops shadow snapshots without blocking the 10 ms host loop, then switches the selected ECU
route at a cycle boundary after revalidating the candidate socket inode. The request succeeds only
after the candidate replies for that promotion cycle and zero output is successfully applied. The
hold remains latched after success. The current host process supports one promotion because it
stops the shadow coordinator while switching a route. Route selection is process-local and returns
to the active worker after a host restart. To confirm that the promoted route keeps completing
held cycles over a stability interval, run:

```sh
sudo sdv-fota verify-candidate --control-socket /run/sdv-fota/control.sock --target engine --stability-ms 2000
```

Each check requires the candidate to answer at a host-cycle boundary and the host to apply zero
output. The command keeps the hold latched and fails if the candidate route or vehicle runtime is
not healthy. This is protocol and host-health evidence only; it does not commit or roll back trial
metadata or verify application behavior. To remove a promoted candidate from the route, use the
root-only reversion command:

```sh
sudo sdv-fota revert-candidate --control-socket /run/sdv-fota/control.sock --target engine
```

Reversion validates the original active worker socket, switches at a host-cycle boundary, and
acknowledges only after that worker replies and zero output is applied. The hold remains latched.
Reversion does not stop the candidate service or change trial metadata; a reverted candidate cannot
be promoted again in the same host process. `Ready`, promotion, and reversion are protocol and route
evidence only; they do not establish application behavior or vehicle health.

If promotion fails, zero output remains held. If the route switched before the failure, request
`revert-candidate` and wait for its active-cycle acknowledgement before assuming the active worker is
selected. If the host cannot complete reversion, keep the vehicle secured; restarting the host with
the trial pending selects the active worker and starts held. Before any promotion, if candidate
startup or shadow validation fails, the active worker stays selected and the hold remains latched.
After the active route is selected, stop the candidate before replacing its package or clearing trial
metadata:

```sh
sudo systemctl stop sdv-fota-ecu-candidate@engine.service
sudo sdv-fota rollback-trial --state-root /var/lib/sdv-fota --target engine
sudo systemctl restart sdv-fota-launcher.service
```

The host restart after a pre-promotion rollback reloads the active vehicle image with no pending
ECU trial and clears the process-local hold. If a pending ECU trial remains at boot, the root launcher sets
`SDV_FOTA_PENDING_ECU_TRIAL=1` and the host starts held before its first runtime cycle. A hold ACK
confirms a zero command was applied; it does not prove physical standstill, so secure the vehicle
before starting the candidate service.

`binary` remains set in `config/pi5.worker-processes.disabled.example.json` for the original
coordinator-spawn mode. Omitting `virtual_ecu_workers` preserves the existing in-process runtime
path.

To switch away from external mode, stop the host and worker target, disable the target, install the
root-owned spawn or in-process vehicle config, then start only the host launcher. To return to
external mode, stop the host and worker target before changing config, confirm each ECU has an
initialized active image, install the external config, start the host launcher, then enable and start
`sdv-fota-workers.target`. Leaving the external config
active while stopping the target makes the host fail closed and restart because its workers are
missing.

Per-ECU FOTA package staging and trial metadata use separate state roots. The candidate service
launches only the pending image into its candidate-only socket, without consuming boots, changing
metadata, or selecting candidate outputs. While it is still shadowing, stop
`sdv-fota-ecu-candidate@<ecu>.service` before installing a replacement or running administrative
rollback; otherwise a shadow process may continue after metadata changes. Output hold is sticky in the host process. Route promotion is
available through `sdv-fota promote-candidate`; root-only `sdv-fota revert-candidate` can return to
the active worker while keeping the hold latched. Trial-metadata rollback is still a separate
administrative operation. Application/vehicle health evidence, trial commit, and hold release remain
unimplemented.

`config/pi5.disabled.example.json` is a commissioning template. Its zero motor duty and zero
forward-power limit keep drive output disabled. `config/pi5.fota.disabled.example.json` uses the
systemd runtime directory for input and health sockets and keeps the existing in-process ECU tasks.
`config/pi5.worker-processes.disabled.example.json` demonstrates opt-in worker processes while
keeping drive output disabled. Replace the GPIO chip placeholder and calibrate motor and steering
values against the target PiCar-X before enabling forward power.

Build and run it with:

```sh
cargo build --release --manifest-path rust/sdv-picarx/Cargo.toml
./rust/sdv-picarx/target/release/sdv-picarx --config /etc/sdv-picarx.json
```

To list available I2C device paths and GPIO chip line offsets without requesting or changing any
pins, build and run `sdv-picarx-discover`:

```sh
cargo run --release --manifest-path rust/sdv-picarx/Cargo.toml --bin sdv-picarx-discover
```

These bindings have passed unit and aarch64 compile checks; they have not been run on a physical
PiCar-X. GPIO mapping, motor polarity, and steering/PWM values still need to be checked and
calibrated on the target vehicle before enabling forward power.

```sh
cargo test --manifest-path rust/sdv-picarx/Cargo.toml
```
