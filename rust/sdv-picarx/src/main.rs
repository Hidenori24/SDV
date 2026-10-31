use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
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
use sdv_picarx::ecu_process_coordinator::{EcuProcessCoordinator, EcuProcessVehicleExecutor};
use sdv_picarx::linux_gpio::LinuxMotorDirections;
use sdv_picarx::linux_i2c::I2cTransport;
use sdv_picarx::linux_input::{Error as InputError, UnixDatagramInputSource};
use sdv_picarx::output_hold::StickyOutputHold;
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
    #[serde(default)]
    virtual_ecu_workers: Option<VirtualEcuWorkerConfig>,
    motor: MotorConfig,
    vehicle: VehicleConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualEcuWorkerConfig {
    #[serde(default)]
    mode: VirtualEcuWorkerMode,
    #[serde(default)]
    binary: Option<PathBuf>,
    socket_directory: PathBuf,
    #[serde(default)]
    candidate_socket_directory: Option<PathBuf>,
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum VirtualEcuWorkerMode {
    #[default]
    Spawn,
    External,
}

impl VirtualEcuWorkerConfig {
    fn is_valid(&self) -> bool {
        let safe_path = |path: &PathBuf| {
            path.is_absolute()
                && !path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
        };
        let mode_matches_binary = match self.mode {
            VirtualEcuWorkerMode::Spawn => {
                self.binary.as_ref().is_some_and(safe_path)
                    && self.candidate_socket_directory.is_none()
            }
            VirtualEcuWorkerMode::External => self.binary.is_none(),
        };
        mode_matches_binary
            && safe_path(&self.socket_directory)
            && self
                .candidate_socket_directory
                .as_ref()
                .map_or(true, |candidate| {
                    safe_path(candidate) && candidate != &self.socket_directory
                })
            && self
                .binary
                .as_ref()
                .map_or(true, |binary| !binary.starts_with(&self.socket_directory))
    }
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
            || self
                .virtual_ecu_workers
                .as_ref()
                .is_some_and(|workers| !workers.is_valid())
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

const HOLD_REQUEST: &[u8] = b"SDVHOLD1\n";
const HOLD_ACK: &[u8] = b"SDVHELD1\n";
const HOLD_NOT_READY: &[u8] = b"SDVNOTREADY\n";
const HOLD_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const HOLD_LISTENER_FD_ENV: &str = "SDV_FOTA_HOLD_LISTENER_FD";

struct HoldControlServer {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HoldControlServer {
    fn from_inherited_fd(
        fd: i32,
        hold: StickyOutputHold,
        health: Arc<RuntimeHealth>,
    ) -> io::Result<Self> {
        if fd < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid inherited hold listener descriptor",
            ));
        }
        let mut socket_type: libc::c_int = 0;
        let mut socket_type_length = std::mem::size_of_val(&socket_type) as libc::socklen_t;
        // SAFETY: `socket_type` is writable and the supplied length is its exact size.
        let type_result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut socket_type as *mut libc::c_int).cast(),
                &mut socket_type_length,
            )
        };
        let mut socket_domain: libc::c_int = 0;
        let mut socket_domain_length = std::mem::size_of_val(&socket_domain) as libc::socklen_t;
        // SAFETY: `socket_domain` is writable and the supplied length is its exact size.
        let domain_result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_DOMAIN,
                (&mut socket_domain as *mut libc::c_int).cast(),
                &mut socket_domain_length,
            )
        };
        let mut accepting: libc::c_int = 0;
        let mut accepting_length = std::mem::size_of_val(&accepting) as libc::socklen_t;
        // SAFETY: `accepting` is writable and the supplied length is its exact size.
        let accepting_result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ACCEPTCONN,
                (&mut accepting as *mut libc::c_int).cast(),
                &mut accepting_length,
            )
        };
        if type_result != 0
            || domain_result != 0
            || accepting_result != 0
            || socket_domain != libc::AF_UNIX
            || socket_type != libc::SOCK_STREAM
            || accepting != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inherited hold descriptor is not a listening stream socket",
            ));
        }
        // SAFETY: the root FOTA launcher passes a listening Unix stream socket descriptor and
        // transfers its child-side ownership to this application process.
        let listener = unsafe { UnixListener::from_raw_fd(fd) };
        // SAFETY: `fd` is an open descriptor owned by `listener`; the calls only inspect and
        // update its descriptor flags.
        let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if descriptor_flags == -1
            || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } == -1
        {
            return Err(io::Error::last_os_error());
        }
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_health = Arc::clone(&health);
        let worker = thread::Builder::new()
            .name("sdv-output-hold-control".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(error) = handle_hold_client(stream, &hold, &thread_health) {
                                eprintln!("sdv-picarx: output hold request rejected: {error}");
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(worker),
        })
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HoldControlServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn handle_hold_client(
    stream: UnixStream,
    hold: &StickyOutputHold,
    health: &RuntimeHealth,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let uid = peer_uid(stream.as_raw_fd())?;
    handle_hold_client_with_uid(stream, uid, hold, health)
}

fn handle_hold_client_with_uid(
    mut stream: UnixStream,
    uid: u32,
    hold: &StickyOutputHold,
    health: &RuntimeHealth,
) -> io::Result<()> {
    if uid != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "hold control is restricted to root",
        ));
    }
    let mut request = [0u8; HOLD_REQUEST.len()];
    if let Err(error) = stream.read_exact(&mut request) {
        let _ = hold.request_and_wait(Duration::ZERO);
        return Err(error);
    }
    let mut trailing = [0u8; 1];
    match stream.read(&mut trailing) {
        Ok(0) => {}
        Ok(_) => {
            let _ = hold.request_and_wait(Duration::ZERO);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hold control request contains trailing bytes",
            ));
        }
        Err(error) => {
            let _ = hold.request_and_wait(Duration::ZERO);
            return Err(error);
        }
    }
    if request != HOLD_REQUEST {
        let _ = hold.request_and_wait(Duration::ZERO);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown hold control request",
        ));
    }
    if !health.is_healthy() {
        stream.write_all(HOLD_NOT_READY)?;
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "vehicle runtime is not ready for an output hold",
        ));
    }
    if !hold.request_and_wait(HOLD_ACK_TIMEOUT) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "zero actuator output was not applied before timeout",
        ));
    }
    stream.write_all(HOLD_ACK)
}

fn peer_uid(fd: std::os::fd::RawFd) -> io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` is a writable buffer of the size supplied to getsockopt.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 || length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: getsockopt succeeded and returned a complete `ucred` structure.
    Ok(unsafe { credentials.assume_init() }.uid)
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
    output_hold: StickyOutputHold,
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
        self.output_hold.apply(
            &mut self.platform,
            if self.trial_mode {
                trial_safe_actuation(actuation)
            } else {
                actuation
            },
        )?;
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

fn ecu_trial_hold_from_env() -> Result<bool, String> {
    match env::var_os("SDV_FOTA_PENDING_ECU_TRIAL") {
        None => Ok(false),
        Some(value) if value == "1" => Ok(true),
        Some(_) => Err("SDV_FOTA_PENDING_ECU_TRIAL must be `1` when set".into()),
    }
}

fn hold_listener_from_env() -> Result<Option<i32>, String> {
    let value = match env::var(HOLD_LISTENER_FD_ENV) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(format!("cannot read {HOLD_LISTENER_FD_ENV}: {error}")),
    };
    parse_hold_listener_fd(value.as_deref())
}

fn parse_hold_listener_fd(value: Option<&str>) -> Result<Option<i32>, String> {
    match value {
        None => Ok(None),
        Some(value) => value
            .parse::<i32>()
            .ok()
            .filter(|fd| *fd >= 3)
            .map(Some)
            .ok_or_else(|| format!("{HOLD_LISTENER_FD_ENV} must identify an inherited descriptor")),
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
    let ecu_trial_hold = ecu_trial_hold_from_env()?;
    let hold_listener_fd = hold_listener_from_env()?;
    env::remove_var(HOLD_LISTENER_FD_ENV);
    env::remove_var("SDV_FOTA_HOLD_SOCKET");
    let config_path = config_path()?;
    let config_data = fs::read(&config_path)
        .map_err(|error| format!("cannot read {}: {error}", config_path.display()))?;
    let config: HostConfig = serde_json::from_slice(&config_data)
        .map_err(|error| format!("invalid configuration: {error}"))?;
    let (motor_calibration, vehicle_calibration) = config.calibrations().map_err(str::to_owned)?;
    let output_hold = StickyOutputHold::new(ecu_trial_hold);

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
    let mut hold_server = hold_listener_fd
        .map(|fd| {
            HoldControlServer::from_inherited_fd(fd, output_hold.clone(), Arc::clone(&health))
        })
        .transpose()
        .map_err(|error| format!("cannot accept inherited output hold socket: {error}"))?;

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
    let mut reporting_platform = HealthReportingPlatform {
        platform,
        health,
        trial_mode,
        output_hold,
    };
    let result = match config.virtual_ecu_workers {
        Some(worker_config) => {
            let coordinator_result = match worker_config.mode {
                VirtualEcuWorkerMode::External => match worker_config.candidate_socket_directory {
                    Some(candidate_directory) => {
                        EcuProcessCoordinator::connect_external_with_candidates(
                            worker_config.socket_directory,
                            candidate_directory,
                        )
                    }
                    None => EcuProcessCoordinator::connect_external(worker_config.socket_directory),
                },
                VirtualEcuWorkerMode::Spawn => EcuProcessCoordinator::spawn(
                    worker_config
                        .binary
                        .expect("spawn binary validated before runtime"),
                    worker_config.socket_directory,
                ),
            };
            let coordinator = match coordinator_result {
                Ok(coordinator) => coordinator,
                Err(error) => {
                    let stop_error = reporting_platform.stop().err();
                    return Err(format!(
                        "cannot connect virtual ECU workers: {error}; motor stop result: {stop_error:?}"
                    ));
                }
            };
            let mut executor = EcuProcessVehicleExecutor::new(
                VehicleRuntime::default(),
                reporting_platform,
                coordinator,
            );
            runtime_host::run(&mut executor, &mut SystemClock::default(), || {
                shutdown.load(Ordering::Relaxed)
            })
            .map_err(|error| format!("vehicle worker runtime stopped: {error:?}"))
        }
        None => {
            let mut executor = VehicleExecutor::new(VehicleRuntime::default(), reporting_platform);
            runtime_host::run(&mut executor, &mut SystemClock::default(), || {
                shutdown.load(Ordering::Relaxed)
            })
            .map_err(|error| format!("vehicle runtime stopped: {error:?}"))
        }
    };
    health_server.shutdown();
    if let Some(server) = &mut hold_server {
        server.shutdown();
    }
    result
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
    use sdv_core::rte::{Safety, SystemState};
    use std::io::BufRead;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

    const VALID_CONFIG: &str = include_str!("../config/pi5.disabled.example.json");

    #[derive(Default)]
    struct HoldTestPlatform {
        applied: Option<VehicleActuation>,
    }

    impl VehiclePlatform for HoldTestPlatform {
        type Error = std::convert::Infallible;

        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            Ok(PlatformInputs::default())
        }

        fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
            self.applied = Some(actuation);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn accepts_named_disabled_commissioning_configuration() {
        let config: HostConfig = serde_json::from_str(VALID_CONFIG).unwrap();
        let (motor, vehicle) = config.calibrations().unwrap();
        assert_eq!(motor.maximum_duty, 0.0);
        assert_eq!(vehicle.max_forward_power, 0.0);

        let fota_config: HostConfig =
            serde_json::from_str(include_str!("../config/pi5.fota.disabled.example.json")).unwrap();
        assert!(fota_config.calibrations().is_ok());
        assert!(fota_config.virtual_ecu_workers.is_none());

        let worker_config: HostConfig = serde_json::from_str(include_str!(
            "../config/pi5.worker-processes.disabled.example.json"
        ))
        .unwrap();
        assert!(worker_config.calibrations().is_ok());
        assert!(worker_config
            .virtual_ecu_workers
            .as_ref()
            .is_some_and(|workers| {
                workers.mode == VirtualEcuWorkerMode::Spawn && workers.binary.is_some()
            }));
        let unsafe_worker_path =
            include_str!("../config/pi5.worker-processes.disabled.example.json")
                .replace("/usr/bin/sdv-picarx-ecu-worker", "../sdv-picarx-ecu-worker");
        let unsafe_worker_config: HostConfig = serde_json::from_str(&unsafe_worker_path).unwrap();
        assert!(unsafe_worker_config.calibrations().is_err());

        let external_config: HostConfig = serde_json::from_str(include_str!(
            "../config/pi5.external-workers.disabled.example.json"
        ))
        .unwrap();
        assert!(external_config.calibrations().is_ok());
        assert!(external_config
            .virtual_ecu_workers
            .as_ref()
            .is_some_and(|workers| {
                workers.mode == VirtualEcuWorkerMode::External
                    && workers.binary.is_none()
                    && workers.candidate_socket_directory
                        == Some(PathBuf::from("/run/sdv-picarx/candidates"))
            }));

        let implicit_spawn_without_binary =
            include_str!("../config/pi5.external-workers.disabled.example.json")
                .replace("\"mode\": \"external\",", "");
        let implicit_spawn_config: HostConfig =
            serde_json::from_str(&implicit_spawn_without_binary).unwrap();
        assert!(implicit_spawn_config.calibrations().is_err());

        let candidate_spawn_config = include_str!("../config/pi5.worker-processes.disabled.example.json")
            .replace(
                "\"socket_directory\": \"/run/sdv-picarx/workers\"",
                "\"socket_directory\": \"/run/sdv-picarx/workers\", \"candidate_socket_directory\": \"/run/sdv-picarx/candidates\"",
            );
        let invalid_candidate_spawn: HostConfig =
            serde_json::from_str(&candidate_spawn_config).unwrap();
        assert!(invalid_candidate_spawn.calibrations().is_err());

        let legacy_spawn_config: HostConfig = serde_json::from_str(include_str!(
            "../config/pi5.worker-processes.disabled.example.json"
        ))
        .unwrap();
        assert!(legacy_spawn_config.calibrations().is_ok());

        let invalid_external_with_binary =
            include_str!("../config/pi5.worker-processes.disabled.example.json").replace(
                "\"virtual_ecu_workers\": {",
                r#""virtual_ecu_workers": { "mode": "external","#,
            );
        let invalid_external_config: HostConfig =
            serde_json::from_str(&invalid_external_with_binary).unwrap();
        assert!(invalid_external_config.calibrations().is_err());
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
    fn inherited_hold_descriptor_requires_a_valid_nonstandard_fd() {
        assert_eq!(parse_hold_listener_fd(None).unwrap(), None);
        assert_eq!(parse_hold_listener_fd(Some("32")).unwrap(), Some(32));
        assert!(parse_hold_listener_fd(Some("0")).is_err());
        assert!(parse_hold_listener_fd(Some("3x")).is_err());
    }

    #[test]
    fn inherited_hold_server_takes_ownership_of_only_a_listening_stream_socket() {
        let path = std::env::temp_dir().join(format!(
            "sdv-picarx-inherited-hold-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let fd = listener.as_raw_fd();
        std::mem::forget(listener);
        let health = Arc::new(RuntimeHealth::new(Duration::from_secs(1), 0.02, 0.0));
        let mut server =
            HoldControlServer::from_inherited_fd(fd, StickyOutputHold::new(false), health.clone())
                .unwrap();
        // SAFETY: `fd` remains owned by the inherited-listener server thread.
        let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        server.shutdown();
        assert!(
            path.exists(),
            "the app must not unlink the root-owned socket path"
        );
        fs::remove_file(path).unwrap();

        let (stream, _peer) = UnixStream::pair().unwrap();
        let error = HoldControlServer::from_inherited_fd(
            stream.as_raw_fd(),
            StickyOutputHold::new(false),
            health,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
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

    fn ready_runtime_health() -> RuntimeHealth {
        let health = RuntimeHealth::new(Duration::from_secs(1), 0.02, 0.0);
        health.record_sample(Some(&PlatformInputs::default()));
        health.mark_cycle();
        health
    }

    #[test]
    fn root_hold_request_before_runtime_ready_returns_not_ready_without_latching() {
        let hold = StickyOutputHold::new(false);
        let server_hold = hold.clone();
        let health = Arc::new(RuntimeHealth::new(Duration::from_secs(1), 0.02, 0.0));
        let thread_health = Arc::clone(&health);
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            handle_hold_client_with_uid(server, 0, &server_hold, &thread_health)
        });
        client.write_all(HOLD_REQUEST).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let mut response = String::new();
        std::io::BufReader::new(client)
            .read_line(&mut response)
            .unwrap();
        assert_eq!(response.as_bytes(), HOLD_NOT_READY);
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!hold.is_held());
    }

    #[test]
    fn root_hold_control_ack_waits_for_successful_zero_apply() {
        let hold = StickyOutputHold::new(false);
        let server_hold = hold.clone();
        let health = Arc::new(ready_runtime_health());
        let thread_health = Arc::clone(&health);
        let (mut client, server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            handle_hold_client_with_uid(server, 0, &server_hold, &thread_health).unwrap();
        });
        client.write_all(HOLD_REQUEST).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let mut ack = [0u8; HOLD_ACK.len()];
        assert!(client.read_exact(&mut ack).is_err());
        assert!(hold.is_held());

        let mut platform = HoldTestPlatform::default();
        hold.apply(
            &mut platform,
            VehicleActuation {
                command: ActuatorCmd {
                    drive_accel_cmd: 1.0,
                    brake_decel_cmd: 0.0,
                    steer_angle_cmd: 0.2,
                },
                safety: Safety {
                    estop: false,
                    system_state: SystemState::Normal,
                },
            },
        )
        .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client.read_exact(&mut ack).unwrap();
        assert_eq!(&ack, HOLD_ACK);
        assert_eq!(platform.applied.unwrap().command, ActuatorCmd::default());
        worker.join().unwrap();
    }

    #[test]
    fn unauthorized_hold_request_does_not_latch_and_malformed_root_request_does() {
        let hold = StickyOutputHold::new(false);
        let health = ready_runtime_health();
        let (client, server) = UnixStream::pair().unwrap();
        assert_eq!(
            handle_hold_client_with_uid(server, 1000, &hold, &health)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(client);
        assert!(!hold.is_held());

        let (mut client, server) = UnixStream::pair().unwrap();
        let handler_hold = hold.clone();
        let worker =
            thread::spawn(move || handle_hold_client_with_uid(server, 0, &handler_hold, &health));
        client.write_all(b"UNKNOWN!!").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(hold.is_held());
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
