//! PiCar-X actuator mapping. Concrete Robot HAT device access is supplied by `RobotHat`.

#[cfg(target_os = "linux")]
pub mod linux_i2c;
#[cfg(target_os = "linux")]
pub mod linux_gpio;
#[cfg(target_os = "linux")]
pub mod linux_input;
pub mod pwm;
pub mod robothat;

use sdv_core::platform::{PlatformInputs, VehiclePlatform};
use sdv_core::rte::SystemState;
use sdv_core::runtime::VehicleActuation;

/// Hardware operations; forward power is normalized to 0..=1, steering is servo degrees.
pub trait RobotHat {
    type Error;
    fn set_steering_degrees(&mut self, angle: f32) -> Result<(), Self::Error>;
    fn set_forward_power(&mut self, power: f32) -> Result<(), Self::Error>;
    fn stop_motors(&mut self) -> Result<(), Self::Error>;
}

/// Supplies fresh driver input and measured or estimated vehicle state.
pub trait InputSource {
    type Error;
    fn sample(&mut self) -> Result<PlatformInputs, Self::Error>;
}

/// Values must be measured for the particular car before driving it.
#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    pub max_drive_accel_mps2: f32,
    pub max_steering_rad: f32,
    pub max_steering_servo_degrees: f32,
    pub steering_center_degrees: f32,
    /// Either 1.0 or -1.0, according to the installed steering linkage.
    pub steering_direction: f32,
    /// Maximum normalized forward power, in 0..=1.
    pub max_forward_power: f32,
}

impl Calibration {
    pub fn is_valid(self) -> bool {
        let values = [
            self.max_drive_accel_mps2,
            self.max_steering_rad,
            self.max_steering_servo_degrees,
            self.steering_center_degrees,
            self.steering_direction,
            self.max_forward_power,
        ];
        values.iter().all(|value| value.is_finite())
            && self.max_drive_accel_mps2 > 0.0
            && self.max_steering_rad > 0.0
            && self.max_steering_servo_degrees > 0.0
            && self.steering_center_degrees.abs() + self.max_steering_servo_degrees <= 90.0
            && (self.steering_direction == 1.0 || self.steering_direction == -1.0)
            && (0.0..=1.0).contains(&self.max_forward_power)
    }
}

#[derive(Debug, PartialEq)]
pub enum Error<I, H> {
    Input(I),
    Hardware(H),
    InvalidActuation,
}

pub struct PiCarX<I, H> {
    inputs: I,
    hat: H,
    calibration: Calibration,
}

impl<I, H> PiCarX<I, H> {
    pub fn new(inputs: I, hat: H, calibration: Calibration) -> Result<Self, &'static str> {
        if !calibration.is_valid() {
            return Err("invalid PiCar-X calibration");
        }
        Ok(Self {
            inputs,
            hat,
            calibration,
        })
    }
    pub fn hardware(&self) -> &H {
        &self.hat
    }
}

impl<I: InputSource, H: RobotHat> VehiclePlatform for PiCarX<I, H> {
    type Error = Error<I::Error, H::Error>;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        self.inputs.sample().map_err(Error::Input)
    }

    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
        let command = actuation.command;
        if !command.drive_accel_cmd.is_finite()
            || !command.brake_decel_cmd.is_finite()
            || !command.steer_angle_cmd.is_finite()
        {
            self.hat.stop_motors().map_err(Error::Hardware)?;
            return Err(Error::InvalidActuation);
        }
        if actuation.safety.estop
            || actuation.safety.system_state == SystemState::EStop
            || command.brake_decel_cmd > 0.0
        {
            return self.hat.stop_motors().map_err(Error::Hardware);
        }
        let steering = self.calibration.steering_center_degrees
            + (command.steer_angle_cmd / self.calibration.max_steering_rad).clamp(-1.0, 1.0)
                * self.calibration.max_steering_servo_degrees
                * self.calibration.steering_direction;
        let power = (command.drive_accel_cmd / self.calibration.max_drive_accel_mps2)
            .clamp(0.0, 1.0)
            * self.calibration.max_forward_power;
        self.hat
            .set_steering_degrees(steering)
            .map_err(Error::Hardware)?;
        self.hat.set_forward_power(power).map_err(Error::Hardware)
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.hat.stop_motors().map_err(Error::Hardware)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdv_core::rte::{ActuatorCmd, Safety};
    use std::convert::Infallible;

    struct Inputs;
    impl InputSource for Inputs {
        type Error = Infallible;
        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            Ok(PlatformInputs::default())
        }
    }
    #[derive(Default)]
    struct Hat {
        power: f32,
        steer: f32,
        stopped: bool,
    }
    impl RobotHat for Hat {
        type Error = Infallible;
        fn set_steering_degrees(&mut self, v: f32) -> Result<(), Self::Error> {
            self.steer = v;
            Ok(())
        }
        fn set_forward_power(&mut self, v: f32) -> Result<(), Self::Error> {
            self.power = v;
            Ok(())
        }
        fn stop_motors(&mut self) -> Result<(), Self::Error> {
            self.stopped = true;
            self.power = 0.0;
            Ok(())
        }
    }
    fn calibration() -> Calibration {
        Calibration {
            max_drive_accel_mps2: 2.0,
            max_steering_rad: 0.4,
            max_steering_servo_degrees: 25.0,
            steering_center_degrees: 2.0,
            steering_direction: -1.0,
            max_forward_power: 0.2,
        }
    }
    #[test]
    fn drive_and_steering_respect_calibrated_limits() {
        let mut car = PiCarX::new(Inputs, Hat::default(), calibration()).unwrap();
        car.apply(VehicleActuation {
            command: ActuatorCmd {
                drive_accel_cmd: 4.0,
                brake_decel_cmd: 0.0,
                steer_angle_cmd: 0.8,
            },
            safety: Safety::default(),
        })
        .unwrap();
        assert_eq!(car.hardware().power, 0.2);
        assert_eq!(car.hardware().steer, -23.0);
    }
    #[test]
    fn braking_and_estop_cut_drive() {
        for safety in [
            Safety::default(),
            Safety {
                estop: true,
                ..Safety::default()
            },
        ] {
            let mut car = PiCarX::new(Inputs, Hat::default(), calibration()).unwrap();
            car.apply(VehicleActuation {
                command: ActuatorCmd {
                    drive_accel_cmd: 1.0,
                    brake_decel_cmd: if safety.estop { 0.0 } else { 0.1 },
                    steer_angle_cmd: 0.0,
                },
                safety,
            })
            .unwrap();
            assert!(car.hardware().stopped);
            assert_eq!(car.hardware().power, 0.0);
        }
    }
    #[test]
    fn unsafe_calibration_is_rejected() {
        let mut config = calibration();
        config.steering_center_degrees = 80.0;
        assert!(PiCarX::new(Inputs, Hat::default(), config).is_err());
    }

    #[test]
    fn invalid_actuator_command_stops_motors() {
        let mut car = PiCarX::new(Inputs, Hat::default(), calibration()).unwrap();
        let actuation = VehicleActuation {
            command: ActuatorCmd {
                drive_accel_cmd: f32::NAN,
                ..ActuatorCmd::default()
            },
            safety: Safety::default(),
        };
        assert!(matches!(car.apply(actuation), Err(Error::InvalidActuation)));
        assert!(car.hardware().stopped);
    }
}
