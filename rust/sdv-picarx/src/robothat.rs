//! PiCar-X steering servo and rear motor binding to Robot HAT PWM and direction pins.

use crate::pwm::{self, Pwm, RegisterTransport};
use crate::RobotHat;

const STEERING_CHANNEL: u8 = 2;
const LEFT_MOTOR_CHANNEL: u8 = 13;
const RIGHT_MOTOR_CHANNEL: u8 = 12;
const PWM_PERIOD: u16 = 4095;
const MOTOR_PRESCALER: u16 = 10;
const SERVO_PRESCALER: u16 = 352;
const SERVO_PERIOD_US: f32 = 20_000.0;
const SERVO_MIN_PULSE_US: f32 = 500.0;
const SERVO_MAX_PULSE_US: f32 = 2_500.0;
const SERVO_LIMIT_DEG: f32 = 90.0;

/// Direction pin operations; implementations must configure pins as outputs before use.
pub trait MotorDirections {
    type Error;
    fn set_left(&mut self, high: bool) -> Result<(), Self::Error>;
    fn set_right(&mut self, high: bool) -> Result<(), Self::Error>;
}

/// Direction polarity and PWM duty limits must be configured for the physical car.
#[derive(Clone, Copy, Debug)]
pub struct MotorCalibration {
    pub left_forward_high: bool,
    pub right_forward_high: bool,
    pub minimum_duty: f32,
    pub maximum_duty: f32,
}

#[derive(Debug)]
pub enum Error<P, G> {
    InvalidCalibration,
    InvalidCommand,
    Pwm(pwm::Error<P>),
    Direction(G),
}

pub struct Actuators<T, G> {
    pwm: Pwm<T>,
    directions: G,
    calibration: MotorCalibration,
}

impl<T: RegisterTransport, G: MotorDirections> Actuators<T, G> {
    /// Stop both motor channels before configuring shared motor and servo timers.
    pub fn initialize(
        transport: T,
        directions: G,
        calibration: MotorCalibration,
    ) -> Result<Self, Error<T::Error, G::Error>> {
        if !calibration.minimum_duty.is_finite()
            || !calibration.maximum_duty.is_finite()
            || !(0.0..=1.0).contains(&calibration.minimum_duty)
            || !(calibration.minimum_duty..=1.0).contains(&calibration.maximum_duty)
        {
            return Err(Error::InvalidCalibration);
        }
        let mut result = Self {
            pwm: Pwm::new(transport),
            directions,
            calibration,
        };
        result.stop_motors()?;
        result
            .pwm
            .configure_timer(LEFT_MOTOR_CHANNEL, MOTOR_PRESCALER, PWM_PERIOD)
            .map_err(Error::Pwm)?;
        result
            .pwm
            .configure_timer(STEERING_CHANNEL, SERVO_PRESCALER, PWM_PERIOD)
            .map_err(Error::Pwm)?;
        Ok(result)
    }
}

impl<T: RegisterTransport, G: MotorDirections> RobotHat for Actuators<T, G> {
    type Error = Error<T::Error, G::Error>;

    fn set_steering_degrees(&mut self, angle: f32) -> Result<(), Self::Error> {
        if !angle.is_finite() || !(-SERVO_LIMIT_DEG..=SERVO_LIMIT_DEG).contains(&angle) {
            return Err(Error::InvalidCommand);
        }
        let fraction = (angle + SERVO_LIMIT_DEG) / (2.0 * SERVO_LIMIT_DEG);
        let pulse_us = SERVO_MIN_PULSE_US + fraction * (SERVO_MAX_PULSE_US - SERVO_MIN_PULSE_US);
        let pulse = (pulse_us / SERVO_PERIOD_US * f32::from(PWM_PERIOD)) as u16;
        self.pwm
            .set_pulse(STEERING_CHANNEL, pulse, PWM_PERIOD)
            .map_err(Error::Pwm)
    }

    fn set_forward_power(&mut self, power: f32) -> Result<(), Self::Error> {
        if !power.is_finite() || !(0.0..=1.0).contains(&power) {
            return Err(Error::InvalidCommand);
        }
        if power == 0.0 {
            return self.stop_motors();
        }
        self.directions
            .set_left(self.calibration.left_forward_high)
            .map_err(Error::Direction)?;
        self.directions
            .set_right(self.calibration.right_forward_high)
            .map_err(Error::Direction)?;
        let duty = self.calibration.minimum_duty
            + power * (self.calibration.maximum_duty - self.calibration.minimum_duty);
        let pulse = (duty * f32::from(PWM_PERIOD)) as u16;
        self.pwm
            .set_pulse(LEFT_MOTOR_CHANNEL, pulse, PWM_PERIOD)
            .map_err(Error::Pwm)?;
        self.pwm
            .set_pulse(RIGHT_MOTOR_CHANNEL, pulse, PWM_PERIOD)
            .map_err(Error::Pwm)
    }

    fn stop_motors(&mut self) -> Result<(), Self::Error> {
        // Always attempt both writes even when one fails.
        let left = self.pwm.set_pulse(LEFT_MOTOR_CHANNEL, 0, PWM_PERIOD);
        let right = self.pwm.set_pulse(RIGHT_MOTOR_CHANNEL, 0, PWM_PERIOD);
        left.and(right).map_err(Error::Pwm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Bus(Rc<RefCell<Vec<[u8; 3]>>>);
    impl RegisterTransport for Bus {
        type Error = ();
        fn write(&mut self, bytes: [u8; 3]) -> Result<(), ()> {
            self.0.borrow_mut().push(bytes);
            Ok(())
        }
    }
    #[derive(Default)]
    struct Pins {
        left: bool,
        right: bool,
    }
    impl MotorDirections for Pins {
        type Error = ();
        fn set_left(&mut self, high: bool) -> Result<(), ()> {
            self.left = high;
            Ok(())
        }
        fn set_right(&mut self, high: bool) -> Result<(), ()> {
            self.right = high;
            Ok(())
        }
    }
    fn config() -> MotorCalibration {
        MotorCalibration {
            left_forward_high: true,
            right_forward_high: false,
            minimum_duty: 0.5,
            maximum_duty: 1.0,
        }
    }

    #[test]
    fn initializes_stopped_and_drives_both_motors() {
        let records = Rc::new(RefCell::new(Vec::new()));
        let mut hat =
            Actuators::initialize(Bus(records.clone()), Pins::default(), config()).unwrap();
        assert_eq!(&records.borrow()[..2], &[[0x2d, 0, 0], [0x2c, 0, 0]]);
        hat.set_steering_degrees(0.0).unwrap();
        hat.set_forward_power(1.0).unwrap();
        assert!(hat.directions.left);
        assert!(!hat.directions.right);
        assert_eq!(
            &records.borrow()[records.borrow().len() - 2..],
            &[[0x2d, 0x0f, 0xff], [0x2c, 0x0f, 0xff]]
        );
        hat.stop_motors().unwrap();
        assert_eq!(
            &records.borrow()[records.borrow().len() - 2..],
            &[[0x2d, 0, 0], [0x2c, 0, 0]]
        );
    }

    #[test]
    fn steering_center_uses_center_pulse() {
        let records = Rc::new(RefCell::new(Vec::new()));
        let mut hat =
            Actuators::initialize(Bus(records.clone()), Pins::default(), config()).unwrap();
        hat.set_steering_degrees(0.0).unwrap();
        assert_eq!(records.borrow().last(), Some(&[0x22, 0x01, 0x33]));
    }

    struct FailFirstWrite {
        writes: usize,
        attempts: Rc<RefCell<Vec<[u8; 3]>>>,
    }

    impl RegisterTransport for FailFirstWrite {
        type Error = ();

        fn write(&mut self, bytes: [u8; 3]) -> Result<(), Self::Error> {
            self.writes += 1;
            self.attempts.borrow_mut().push(bytes);
            if self.writes == 1 {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn stop_attempts_both_motor_channels_after_a_write_error() {
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let transport = FailFirstWrite {
            writes: 0,
            attempts: attempts.clone(),
        };
        let mut hat = Actuators {
            pwm: Pwm::new(transport),
            directions: Pins::default(),
            calibration: config(),
        };

        assert!(matches!(
            hat.stop_motors(),
            Err(Error::Pwm(pwm::Error::Transport(())))
        ));
        assert_eq!(*attempts.borrow(), vec![[0x2d, 0, 0], [0x2c, 0, 0]]);
    }
}
