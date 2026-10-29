# PiCar-X platform adapter

This crate maps SDV virtual ECU commands to a replaceable `RobotHat` interface. Supply a fresh
`InputSource`, a hardware implementation, and calibration values measured for the car. The
adapter limits forward power and steering travel; emergency stop and any positive brake request
stop the drive motors. Cutting motor power does not guarantee a particular deceleration.

The mapping is open loop: requested acceleration is scaled to normalized forward power. It does
not estimate speed, implement a mechanical brake, or calibrate the car. The Robot HAT GPIO driver,
motor/servo binding, and Raspberry Pi executable are not yet implemented or validated on hardware.

`pwm` provides the Robot HAT MCU PWM register framing behind a `RegisterTransport` interface.
Channels share timers, so a hardware adapter must coordinate their frequency and period. A failed
timer configuration can leave a partial write; the caller must stop the motors and reinitialize
before proceeding. The register mapping was checked against [SunFounder's Robot HAT repository](https://github.com/sunfounder/robot-hat).

On Linux, `linux_i2c::I2cTransport` opens an explicitly configured i2c-dev path and MCU address,
then sends each PWM register message as one I2C transaction. This transport is not yet connected
to a Robot HAT motor or steering implementation and has not been tested on a physical Pi.

```sh
cargo test --manifest-path rust/sdv-picarx/Cargo.toml
```
