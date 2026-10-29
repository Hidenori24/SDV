use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sdv_core::platform::{PlatformInputs, VehicleExecutor, VehiclePlatform};
use sdv_core::rte::ActuatorCmd;
use sdv_core::runtime::VehicleActuation;
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
    health_socket: PathBuf,
    input_max_age_ms: u64,
    trial_stationary_speed_tolerance_mps: f32,
    trial_stationary_throttle_tolerance: f32,
    motor: MotorConfig,
    vehicle: VehicleConfig,
}

impl HostConfig {
    fn calibrations(&self) -> Result<(MotorCalibration, Calibration), &'static str> {
        let motor = MotorCalibration::from(self.motor);
        let vehicle = Calibration::from(self.vehicle);
        if self.input_max_age_ms == 0
            || !self.trial_stationary_speed_tolerance_mps.is_finite()
            || self.trial_stationary_speed_tolerance_mps < 0.0
            || !self.trial_stationary_throttle_tolerance.is_finite()
            || self.trial_stationary_throttle_tolerance < 0.0
            || self.left_direction_line == self.right_direction_line
            || self.input_socket == self.health_socket
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

struct RuntimeHealth {
    started_at: Instant,
    last_cycle_ms: std::sync::atomic::AtomicU64,
    saw_cycle: AtomicBool,
    stationary: AtomicBool,
    max_age: Duration,
    stationary_speed_tolerance_mps: f32,
    stationary_throttle_tolerance: f32,
}

impl RuntimeHealth {
    fn new(
        max_age: Duration,
        stationary_speed_tolerance_mps: f32,
        stationary_throttle_tolerance: f32,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            last_cycle_ms: std::sync::atomic::AtomicU64::new(0),
            saw_cycle: AtomicBool::new(false),
            stationary: AtomicBool::new(false),
            max_age,
            stationary_speed_tolerance_mps,
            stationary_throttle_tolerance,
        }
    }

    fn record_sample(&self, inputs: Option<&PlatformInputs>) {
        let stationary = inputs.is_some_and(|inputs| {
            inputs.vehicle.v.abs() <= self.stationary_speed_tolerance_mps
                && inputs.driver.throttle.abs() <= self.stationary_throttle_tolerance
        });
        self.stationary.store(stationary, Ordering::Release);
    }

    fn mark_cycle(&self) {
        let elapsed_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_cycle_ms.store(elapsed_ms, Ordering::Relaxed);
        self.saw_cycle.store(true, Ordering::Release);
    }

    fn is_healthy(&self) -> bool {
        if !self.saw_cycle.load(Ordering::Acquire) || !self.stationary.load(Ordering::Acquire) {
            return false;
        }
        let now_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let last_cycle_ms = self.last_cycle_ms.load(Ordering::Relaxed);
        now_ms.saturating_sub(last_cycle_ms) <= self.max_age.as_millis() as u64
    }
}

struct HealthServer {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
}

impl HealthServer {
    fn bind(path: PathBuf, health: Arc<RuntimeHealth>) -> io::Result<Self> {
        let listener = UnixListener::bind(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "health endpoint is not a socket",
            ));
        }
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("sdv-picarx-health".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let status = if health.is_healthy() {
                                b"READY\n".as_slice()
                            } else {
                                b"NOT_READY\n".as_slice()
                            };
                            let _ = stream.write_all(status);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(ECU_PERIOD);
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(worker),
            socket_path: path,
            socket_identity: (metadata.dev(), metadata.ino()),
        })
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path) {
            if metadata.file_type().is_socket()
                && (metadata.dev(), metadata.ino()) == self.socket_identity
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }
}

impl Drop for HealthServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct HealthReportingPlatform<P> {
    platform: P,
    health: Arc<RuntimeHealth>,
    trial_mode: bool,
}

impl<P: VehiclePlatform> VehiclePlatform for HealthReportingPlatform<P> {
    type Error = P::Error;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        match self.platform.sample() {
            Ok(inputs) => {
                self.health.record_sample(Some(&inputs));
                Ok(inputs)
            }
            Err(error) => {
                self.health.record_sample(None);
                Err(error)
            }
        }
    }

    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
        self.platform.apply(if self.trial_mode {
            trial_safe_actuation(actuation)
        } else {
            actuation
        })?;
        self.health.mark_cycle();
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.platform.stop()
    }
}

fn trial_safe_actuation(actuation: VehicleActuation) -> VehicleActuation {
    VehicleActuation {
        command: ActuatorCmd::default(),
        safety: actuation.safety,
    }
}

fn trial_mode_from_env() -> Result<bool, String> {
    match env::var_os("SDV_TRIAL_SLOT") {
        None => Ok(false),
        Some(slot) if slot == "a" || slot == "b" => Ok(true),
        Some(_) => Err("SDV_TRIAL_SLOT must be `a` or `b`".into()),
    }
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
    let trial_mode = trial_mode_from_env()?;
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

    let health = Arc::new(RuntimeHealth::new(
        Duration::from_millis(config.input_max_age_ms),
        config.trial_stationary_speed_tolerance_mps,
        config.trial_stationary_throttle_tolerance,
    ));
    let mut health_server = HealthServer::bind(config.health_socket, Arc::clone(&health))
        .map_err(|error| format!("cannot bind vehicle health socket: {error}"))?;

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
    let reporting_platform = HealthReportingPlatform {
        platform,
        health,
        trial_mode,
    };
    let mut executor = VehicleExecutor::new(VehicleRuntime::default(), reporting_platform);
    let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || {
        shutdown.load(Ordering::Relaxed)
    });
    health_server.shutdown();
    result.map_err(|error| format!("vehicle runtime stopped: {error:?}"))
}

fn config_path() -> Result<PathBuf, String> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    config_path_from_args(program, args, env::var_os("SDV_PICARX_CONFIG"))
}

fn config_path_from_args(
    program: std::ffi::OsString,
    mut args: impl Iterator<Item = std::ffi::OsString>,
    environment_path: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => Ok(PathBuf::from(path)),
        (None, None, None) => environment_path
            .map(PathBuf::from)
            .ok_or_else(|| "set SDV_PICARX_CONFIG or pass --config <file.json>".into()),
        _ => Err(format!(
            "usage: {} --config <file.json>",
            PathBuf::from(program).display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

    const VALID_CONFIG: &str = include_str!("../config/pi5.disabled.example.json");

    #[test]
    fn accepts_named_disabled_commissioning_configuration() {
        let config: HostConfig = serde_json::from_str(VALID_CONFIG).unwrap();
        let (motor, vehicle) = config.calibrations().unwrap();
        assert_eq!(motor.maximum_duty, 0.0);
        assert_eq!(vehicle.max_forward_power, 0.0);

        let fota_config: HostConfig =
            serde_json::from_str(include_str!("../config/pi5.fota.disabled.example.json")).unwrap();
        assert!(fota_config.calibrations().is_ok());
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

        let invalid_stationary_limit = VALID_CONFIG.replace(
            "\"trial_stationary_speed_tolerance_mps\": 0.02",
            "\"trial_stationary_speed_tolerance_mps\": -0.02",
        );
        let config: HostConfig = serde_json::from_str(&invalid_stationary_limit).unwrap();
        assert!(config.calibrations().is_err());
    }

    #[test]
    fn fota_environment_supplies_config_when_image_has_no_arguments() {
        let path = config_path_from_args(
            "sdv-picarx".into(),
            std::iter::empty(),
            Some("/etc/sdv-picarx/vehicle.json".into()),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/etc/sdv-picarx/vehicle.json"));
    }

    #[test]
    fn health_socket_requires_a_recent_successful_control_cycle() {
        let path = std::env::temp_dir().join(format!(
            "sdv-picarx-health-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let health = Arc::new(RuntimeHealth::new(Duration::from_secs(1), 0.02, 0.0));
        let mut server = HealthServer::bind(path.clone(), Arc::clone(&health)).unwrap();

        let mut response = String::new();
        let mut stream = std::io::BufReader::new(UnixStream::connect(&path).unwrap());
        stream.read_line(&mut response).unwrap();
        assert_eq!(response, "NOT_READY\n");

        health.record_sample(Some(&PlatformInputs::default()));
        health.mark_cycle();
        response.clear();
        let mut stream = std::io::BufReader::new(UnixStream::connect(&path).unwrap());
        stream.read_line(&mut response).unwrap();
        assert_eq!(response, "READY\n");

        let moving = PlatformInputs {
            vehicle: sdv_core::model::VehicleState {
                v: 0.1,
                ..Default::default()
            },
            ..Default::default()
        };
        health.record_sample(Some(&moving));
        response.clear();
        let mut stream = std::io::BufReader::new(UnixStream::connect(&path).unwrap());
        stream.read_line(&mut response).unwrap();
        assert_eq!(response, "NOT_READY\n");

        health.record_sample(Some(&PlatformInputs::default()));

        server.shutdown();
        assert!(!path.exists());
    }

    #[test]
    fn health_expires_when_control_cycles_stop() {
        let health = RuntimeHealth::new(Duration::from_nanos(1), 0.02, 0.0);
        health.record_sample(Some(&PlatformInputs::default()));
        health.mark_cycle();
        thread::sleep(Duration::from_millis(2));
        assert!(!health.is_healthy());
    }

    #[test]
    fn health_rejects_nonzero_throttle_even_when_estimated_speed_is_zero() {
        let health = RuntimeHealth::new(Duration::from_secs(1), 0.02, 0.0);
        health.record_sample(Some(&PlatformInputs {
            driver: sdv_core::rte::DriverInput {
                throttle: 0.1,
                ..Default::default()
            },
            ..Default::default()
        }));
        health.mark_cycle();
        assert!(!health.is_healthy());
    }

    #[test]
    fn trial_actuation_suppresses_commands_and_preserves_safety_state() {
        let actuation = VehicleActuation {
            command: ActuatorCmd {
                drive_accel_cmd: 1.0,
                brake_decel_cmd: 0.5,
                steer_angle_cmd: 0.25,
            },
            safety: sdv_core::rte::Safety {
                estop: true,
                ..Default::default()
            },
        };

        let safe = trial_safe_actuation(actuation);

        assert_eq!(safe.command, ActuatorCmd::default());
        assert_eq!(safe.safety, actuation.safety);
    }
}
