# PiCar-X platform adapter

This crate maps SDV virtual ECU commands to a replaceable `RobotHat` interface. Supply a fresh
`InputSource`, a hardware implementation, and calibration values measured for the car. The
adapter limits forward power and steering travel; emergency stop and any positive brake request
stop the drive motors. Cutting motor power does not guarantee a particular deceleration.

The mapping is open loop: requested acceleration is scaled to normalized forward power. It does
not estimate speed, implement a mechanical brake, or calibrate the car. The Robot HAT I2C/GPIO
driver and Raspberry Pi executable are not yet implemented or validated on hardware.

```sh
cargo test --manifest-path rust/sdv-picarx/Cargo.toml
```
