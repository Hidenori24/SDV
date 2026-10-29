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

On Linux, `linux_i2c::I2cTransport` opens an explicitly configured i2c-dev path and MCU address,
then sends each PWM register message as one I2C transaction. The current crate still needs a Linux
GPIO implementation, a fresh input source, and an executable that composes these ports. These
bindings have passed unit and aarch64 compile checks; they have not been run on a physical PiCar-X.

```sh
cargo test --manifest-path rust/sdv-picarx/Cargo.toml
```
