use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sdv_core::platform::VehicleExecutor;
use sdv_core::runtime::VehicleRuntime;
use sdv_core::runtime_host::{self, SystemClock, ECU_PERIOD};
use sdv_picarx::linux_gpio::LinuxMotorDirections;
use sdv_picarx::linux_i2c::I2cTransport;
use sdv_picarx::linux_input::{Error as InputError, UnixDatagramInputSource};
use sdv_picarx::robothat::{Actuators, MotorCalibration};
use sdv_picarx::{Calibration, InputSource, PiCarX, RobotHat};
use serde::Deserialize;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfig {
    i2c_device: PathBuf,
    robot_hat_address: u16,
    gpio_chip: PathBuf,
    left_direction_line: u32,
    right_direction_line: u32,
    input_socket: PathBuf,
    input_max_age_ms: u64,
    motor: MotorConfig,
    vehicle: VehicleConfig,
}

impl HostConfig {
    fn calibrations(&self) -> Result<(MotorCalibration, Calibration), &'static str> {
        let motor = MotorCalibration::from(self.motor);
        let vehicle = Calibration::from(self.vehicle);
        if self.input_max_age_ms == 0
            || self.left_direction_line == self.right_direction_line
            || !motor.is_valid()
            || !vehicle.is_valid()
        {
            return Err("invalid input, GPIO, or actuator calibration settings");
        }
        Ok((motor, vehicle))
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct MotorConfig {
    left_forward_high: bool,
    right_forward_high: bool,
    minimum_duty: f32,
    maximum_duty: f32,
}

impl From<MotorConfig> for MotorCalibration {
    fn from(config: MotorConfig) -> Self {
        Self {
            left_forward_high: config.left_forward_high,
            right_forward_high: config.right_forward_high,
            minimum_duty: config.minimum_duty,
            maximum_duty: config.maximum_duty,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct VehicleConfig {
    max_drive_accel_mps2: f32,
    max_steering_rad: f32,
    max_steering_servo_degrees: f32,
    steering_center_degrees: f32,
    steering_direction: f32,
    max_forward_power: f32,
}

impl From<VehicleConfig> for Calibration {
    fn from(config: VehicleConfig) -> Self {
        Self {
            max_drive_accel_mps2: config.max_drive_accel_mps2,
            max_steering_rad: config.max_steering_rad,
            max_steering_servo_degrees: config.max_steering_servo_degrees,
            steering_center_degrees: config.steering_center_degrees,
            steering_direction: config.steering_direction,
            max_forward_power: config.max_forward_power,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sdv-picarx: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config_path = config_path()?;
    let config_data = fs::read(&config_path)
        .map_err(|error| format!("cannot read {}: {error}", config_path.display()))?;
    let config: HostConfig = serde_json::from_slice(&config_data)
        .map_err(|error| format!("invalid configuration: {error}"))?;
    let (motor_calibration, vehicle_calibration) = config.calibrations().map_err(str::to_owned)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, Arc::clone(&shutdown))
        .map_err(|error| format!("cannot register SIGINT handler: {error}"))?;
    flag::register(SIGTERM, Arc::clone(&shutdown))
        .map_err(|error| format!("cannot register SIGTERM handler: {error}"))?;

    let i2c = I2cTransport::open(&config.i2c_device, config.robot_hat_address)
        .map_err(|error| format!("cannot open Robot HAT I2C: {error}"))?;
    let directions = LinuxMotorDirections::open(
        &config.gpio_chip,
        config.left_direction_line,
        config.right_direction_line,
    )
    .map_err(|error| format!("cannot open motor direction GPIO lines: {error:?}"))?;
    let mut actuators = Actuators::initialize(i2c, directions, motor_calibration)
        .map_err(|error| format!("cannot initialize Robot HAT actuators: {error:?}"))?;

    let mut inputs = UnixDatagramInputSource::bind(
        &config.input_socket,
        Duration::from_millis(config.input_max_age_ms),
    )
    .map_err(|error| format!("cannot bind vehicle input socket: {error:?}"))?;
    while !shutdown.load(Ordering::Relaxed) {
        match inputs.sample() {
            Ok(_) => break,
            Err(InputError::NoInput) => thread::sleep(ECU_PERIOD),
            Err(error) => {
                let _ = actuators.stop_motors();
                return Err(format!("invalid vehicle input during startup: {error:?}"));
            }
        }
    }
    if shutdown.load(Ordering::Relaxed) {
        return actuators
            .stop_motors()
            .map_err(|error| format!("shutdown motor stop failed: {error:?}"));
    }

    let platform = PiCarX::new(inputs, actuators, vehicle_calibration)
        .map_err(|error| format!("cannot initialize PiCar-X platform: {error}"))?;
    let mut executor = VehicleExecutor::new(VehicleRuntime::default(), platform);
    runtime_host::run(&mut executor, &mut SystemClock::default(), || {
        shutdown.load(Ordering::Relaxed)
    })
    .map_err(|error| format!("vehicle runtime stopped: {error:?}"))
}

fn config_path() -> Result<PathBuf, String> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => Ok(PathBuf::from(path)),
        _ => Err(format!(
            "usage: {} --config <file.json>",
            PathBuf::from(program).display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = include_str!("../config/pi5.disabled.example.json");

    #[test]
    fn accepts_named_disabled_commissioning_configuration() {
        let config: HostConfig = serde_json::from_str(VALID_CONFIG).unwrap();
        let (motor, vehicle) = config.calibrations().unwrap();
        assert_eq!(motor.maximum_duty, 0.0);
        assert_eq!(vehicle.max_forward_power, 0.0);
    }

    #[test]
    fn rejects_unknown_settings_and_duplicate_direction_lines() {
        let unknown = VALID_CONFIG.replace("\"robot_hat_address\": 20,", "\"arg2\": 20,");
        assert!(serde_json::from_str::<HostConfig>(&unknown).is_err());

        let duplicate = VALID_CONFIG.replace(
            "\"right_direction_line\": 24",
            "\"right_direction_line\": 23",
        );
        let config: HostConfig = serde_json::from_str(&duplicate).unwrap();
        assert!(config.calibrations().is_err());
    }
}
