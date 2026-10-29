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
over a local Unix datagram socket. It accepts only increasing sequence numbers and returns an error
when no sample has arrived within the configured freshness limit; `VehicleExecutor` then requests
a motor stop. Vehicle state must come from a sensor or estimator process. The packet shape is:

```json
{"sequence":1,"throttle":0.4,"brake":0.0,"steer":-0.1,"estop":false,"vehicle":{"t":1.0,"x":2.0,"y":3.0,"yaw":0.2,"v":0.5,"yaw_rate":0.1,"wheel_omega":5.0}}
```

## Linux vehicle host

The `sdv-picarx` executable composes the I2C, GPIO, input, PiCar-X, and virtual ECU runtime ports.
It accepts one named option, `--config <file.json>`. The JSON config contains:

| Field | Meaning |
| --- | --- |
| `i2c_device`, `robot_hat_address` | Linux I2C device path and 7-bit Robot HAT address |
| `gpio_chip`, `left_direction_line`, `right_direction_line` | GPIO chip device path and line offsets |
| `input_socket`, `input_max_age_ms` | Local input socket path and maximum input age |
| `motor` | Forward direction polarity and measured minimum/maximum PWM duty |
| `vehicle` | Measured drive and steering calibration used by the ECU adapter |

The host validates calibration before opening devices, initializes the motor PWM to zero, and waits
for a first valid input packet before starting the 10 ms ECU loop. Missing or stale input, hardware
I/O failure, a missed ECU deadline, SIGINT, and SIGTERM request a motor stop. The process removes
its input socket path on exit. Vehicle state in each packet must be supplied by a real sensor or
state estimator; this crate does not provide one. A health socket reports ready only after a
successful actuator cycle and turns unhealthy if successful cycles stop for longer than the input
freshness limit. The `sdv-picarx-health --socket <path>` command is suitable for the FOTA launcher's
trial check. With no command line arguments, the vehicle executable reads its config path from
`SDV_PICARX_CONFIG`, as used by the FOTA systemd service.

`config/pi5.disabled.example.json` is a commissioning template. Its zero motor duty and zero
forward-power limit keep drive output disabled. `config/pi5.fota.disabled.example.json` uses the
systemd runtime directory for input and health sockets. Replace the GPIO chip placeholder and
calibrate motor and steering values against the target PiCar-X before enabling forward power.

Build and run it with:

```sh
cargo build --release --manifest-path rust/sdv-picarx/Cargo.toml
./rust/sdv-picarx/target/release/sdv-picarx --config /etc/sdv-picarx.json
```

These bindings have passed unit and aarch64 compile checks; they have not been run on a physical
PiCar-X. GPIO mapping, motor polarity, and steering/PWM values still need to be checked and
calibrated on the target vehicle before enabling forward power.

```sh
cargo test --manifest-path rust/sdv-picarx/Cargo.toml
```
