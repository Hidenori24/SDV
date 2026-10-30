//! Pi-side owner for the Engine, Brake, and Steering worker processes.
//!
//! The coordinator owns the single reply socket. It sends all three ECU requests for a cycle
//! before waiting, then accepts exactly one response from each expected worker with the same
//! cycle ID. It does not own sensor or actuator I/O; the vehicle platform remains the only I/O
//! owner.

use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sdv_core::bus::Frame;
use sdv_core::platform::{HostCycleExecutor, VehiclePlatform};
use sdv_core::runtime::{EcuTaskOutput, ExternalEcuCycleError, VehicleRuntime, VirtualEcuId};

use crate::virtual_ecu_worker::{
    decode_response, encode_request, WorkerError, WORKER_DATAGRAM_MAX_SIZE,
};

/// Platform or worker failure during an externally dispatched 10 ms cycle.
#[derive(Debug)]
pub enum VehicleCycleError<E> {
    Sample {
        source: E,
        stop_error: Option<E>,
    },
    InvalidInput {
        stop_error: Option<E>,
    },
    Workers {
        source: ExternalEcuCycleError<Error>,
        stop_error: Option<E>,
    },
    DeadlineExceeded {
        stop_error: Option<E>,
    },
    Apply {
        source: E,
        stop_error: Option<E>,
    },
    Stop(E),
}

/// A Pi vehicle executor whose only difference from `VehicleExecutor` is that ECU task cycles
/// are dispatched to the three local worker processes.
pub struct EcuProcessVehicleExecutor<P> {
    runtime: VehicleRuntime,
    platform: P,
    coordinator: EcuProcessCoordinator,
}

impl<P> EcuProcessVehicleExecutor<P> {
    pub fn new(runtime: VehicleRuntime, platform: P, coordinator: EcuProcessCoordinator) -> Self {
        Self {
            runtime,
            platform,
            coordinator,
        }
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }

    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }
}

impl<P: VehiclePlatform> HostCycleExecutor for EcuProcessVehicleExecutor<P> {
    type Error = VehicleCycleError<P::Error>;

    fn host_cycle(&mut self, remaining_budget: Duration) -> Result<(), Self::Error> {
        let deadline = Instant::now()
            .checked_add(remaining_budget)
            .ok_or_else(|| VehicleCycleError::DeadlineExceeded {
                stop_error: self.platform.stop().err(),
            })?;
        let inputs = match self.platform.sample() {
            Ok(inputs) => inputs,
            Err(source) => {
                return Err(VehicleCycleError::Sample {
                    source,
                    stop_error: self.platform.stop().err(),
                })
            }
        };
        if !inputs.is_finite() {
            return Err(VehicleCycleError::InvalidInput {
                stop_error: self.platform.stop().err(),
            });
        }

        self.runtime.set_estop_request(inputs.estop);
        let coordinator = &mut self.coordinator;
        let actuation = match self.runtime.step_10ms_with_external_ecus(
            inputs.driver,
            inputs.vehicle,
            true,
            |_, ecu_inputs| {
                let cycle_id = coordinator.next_cycle_id().map_err(|error| error)?;
                coordinator.exchange_cycle(cycle_id, ecu_inputs, deadline)
            },
        ) {
            Ok(actuation) => actuation,
            Err(source) => {
                return Err(VehicleCycleError::Workers {
                    source,
                    stop_error: self.platform.stop().err(),
                })
            }
        };
        if Instant::now() >= deadline {
            return Err(VehicleCycleError::DeadlineExceeded {
                stop_error: self.platform.stop().err(),
            });
        }
        if let Err(source) = self.platform.apply(actuation) {
            return Err(VehicleCycleError::Apply {
                source,
                stop_error: self.platform.stop().err(),
            });
        }
        Ok(())
    }

    fn host_stop(&mut self) -> Result<(), Self::Error> {
        self.platform.stop().map_err(VehicleCycleError::Stop)
    }
}

const SOCKET_MODE: u32 = 0o600;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const WORKER_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const EXTERNAL_WORKER_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const WORKER_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(1);
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const COORDINATOR_SOCKET_NAME: &str = "coordinator.sock";
const WORKER_SOCKET_NAMES: [&str; 3] = ["engine.sock", "brake.sock", "steering.sock"];
const ECU_IDS: [VirtualEcuId; 3] = [
    VirtualEcuId::Engine,
    VirtualEcuId::Brake,
    VirtualEcuId::Steering,
];

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    WorkerProtocol(WorkerError),
    UnsafeSocketPath,
    UnsafeWorkerExecutable(PathBuf),
    SocketPathInUse(PathBuf),
    WorkerStartupTimeout(VirtualEcuId),
    WorkerExited {
        ecu: VirtualEcuId,
        status: std::process::ExitStatus,
    },
    InvalidWorkerSocket(VirtualEcuId),
    UnknownPeer,
    WrongWorkerEcu,
    WrongCycleId,
    DuplicateResponse(VirtualEcuId),
    MissingResponse(VirtualEcuId),
    DeadlineExceeded,
    MonotonicClock(io::Error),
    CycleIdExhausted,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "worker coordinator I/O failed: {error}"),
            Self::WorkerProtocol(error) => write!(formatter, "invalid ECU worker packet: {error}"),
            Self::UnsafeSocketPath => formatter.write_str("unsafe ECU worker socket path"),
            Self::UnsafeWorkerExecutable(path) => write!(
                formatter,
                "worker executable is not a trusted regular executable: {}",
                path.display()
            ),
            Self::SocketPathInUse(path) => {
                write!(
                    formatter,
                    "ECU worker socket path already exists: {}",
                    path.display()
                )
            }
            Self::WorkerStartupTimeout(ecu) => {
                write!(formatter, "{ecu:?} worker socket did not become ready")
            }
            Self::WorkerExited { ecu, status } => {
                write!(formatter, "{ecu:?} worker exited with status {status}")
            }
            Self::InvalidWorkerSocket(ecu) => {
                write!(
                    formatter,
                    "{ecu:?} worker socket has unsafe type, owner, or mode"
                )
            }
            Self::UnknownPeer => formatter.write_str("response came from an unknown worker socket"),
            Self::WrongWorkerEcu => {
                formatter.write_str("response ECU does not match worker socket")
            }
            Self::WrongCycleId => {
                formatter.write_str("worker response has a stale or future cycle ID")
            }
            Self::DuplicateResponse(ecu) => write!(formatter, "duplicate response from {ecu:?}"),
            Self::MissingResponse(ecu) => write!(formatter, "missing response from {ecu:?}"),
            Self::DeadlineExceeded => formatter.write_str("ECU worker cycle deadline exceeded"),
            Self::MonotonicClock(error) => write!(formatter, "monotonic clock failed: {error}"),
            Self::CycleIdExhausted => formatter.write_str("ECU worker cycle ID exhausted"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) | Self::MonotonicClock(error) => Some(error),
            Self::WorkerProtocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WorkerError> for Error {
    fn from(error: WorkerError) -> Self {
        Self::WorkerProtocol(error)
    }
}

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

struct SocketPathGuard {
    path: PathBuf,
    owner: u32,
    identity: Option<SocketIdentity>,
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        if let Some(identity) = self.identity {
            remove_socket_if_same(&self.path, self.owner, &identity);
        }
    }
}

struct WorkerProcess {
    ecu: VirtualEcuId,
    socket_path: PathBuf,
    socket_identity: Option<SocketIdentity>,
    child: Option<Child>,
}

/// Owns one Unix datagram reply socket and coordinates one worker per virtual ECU.
pub struct EcuProcessCoordinator {
    socket: UnixDatagram,
    socket_path: PathBuf,
    socket_identity: SocketIdentity,
    socket_directory: PathBuf,
    socket_directory_identity: SocketIdentity,
    owner: u32,
    workers: Vec<WorkerProcess>,
    owns_workers: bool,
    last_cycle_id: Option<u64>,
}

impl EcuProcessCoordinator {
    /// Start the three ECU workers. The socket directory must be absolute, private, owned by
    /// this process, and have a safe owner/root-only ancestor chain. If it does not exist, only
    /// its final component is created with mode 0700.
    pub fn spawn(
        worker_binary: impl AsRef<Path>,
        socket_directory: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        let owner = unsafe { libc::geteuid() };
        validate_worker_executable(worker_binary.as_ref(), owner)?;
        let socket_directory = socket_directory.as_ref().to_path_buf();
        prepare_socket_directory(&socket_directory, owner)?;
        let socket_directory_identity = directory_identity(&socket_directory, owner)?;
        let startup_deadline = Instant::now() + WORKER_STARTUP_TIMEOUT;

        let mut coordinator = Self::bind_coordinator_socket(
            socket_directory.clone(),
            socket_directory_identity,
            owner,
            true,
        )?;

        for (ecu, socket_name) in ECU_IDS.into_iter().zip(WORKER_SOCKET_NAMES) {
            let worker_socket = socket_directory.join(socket_name);
            ensure_path_absent(&worker_socket)?;
            let parent_pid = unsafe { libc::getpid() };
            let mut command = Command::new(worker_binary.as_ref());
            command
                .arg("--ecu")
                .arg(ecu_name(ecu))
                .arg("--worker-socket")
                .arg(&worker_socket)
                .arg("--coordinator-socket")
                .arg(&coordinator.socket_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            // Ensure a worker does not remain orphaned if the coordinator is killed before it can
            // run its Drop cleanup. The parent check closes the race before PR_SET_PDEATHSIG.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() != parent_pid {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "coordinator exited while worker was starting",
                        ));
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            coordinator.workers.push(WorkerProcess {
                ecu,
                socket_path: worker_socket,
                socket_identity: None,
                child: Some(child),
            });
            // Capture each worker's socket identity before starting the next process. If a later
            // spawn/readiness check fails, Drop can reap this child and remove only its socket.
            coordinator.wait_for_worker(coordinator.workers.len() - 1, startup_deadline)?;
        }
        Ok(coordinator)
    }

    /// Connect to the three independently managed worker sockets in `socket_directory`.
    ///
    /// The coordinator creates and owns only `coordinator.sock`; worker processes and their
    /// socket inodes remain owned by their service managers. Each worker socket must be an
    /// owner-only Unix datagram socket created by this effective UID in the validated private
    /// socket directory.
    pub fn connect_external(socket_directory: impl AsRef<Path>) -> Result<Self, Error> {
        let owner = unsafe { libc::geteuid() };
        let socket_directory = socket_directory.as_ref().to_path_buf();
        let socket_directory_identity =
            validate_existing_socket_directory(&socket_directory, owner)?;
        let mut coordinator = Self::bind_coordinator_socket(
            socket_directory,
            socket_directory_identity,
            owner,
            false,
        )?;
        let startup_deadline = Instant::now() + EXTERNAL_WORKER_STARTUP_TIMEOUT;

        for (ecu, socket_name) in ECU_IDS.into_iter().zip(WORKER_SOCKET_NAMES) {
            coordinator.workers.push(WorkerProcess {
                ecu,
                socket_path: coordinator.socket_directory.join(socket_name),
                socket_identity: None,
                child: None,
            });
            coordinator
                .wait_for_external_worker(coordinator.workers.len() - 1, startup_deadline)?;
        }
        Ok(coordinator)
    }

    fn bind_coordinator_socket(
        socket_directory: PathBuf,
        socket_directory_identity: SocketIdentity,
        owner: u32,
        owns_workers: bool,
    ) -> Result<Self, Error> {
        let socket_path = socket_directory.join(COORDINATOR_SOCKET_NAME);
        ensure_path_absent(&socket_path)?;
        let socket = UnixDatagram::bind(&socket_path)?;
        let identity = socket_identity(&socket_path, owner)?;
        let mut socket_guard = SocketPathGuard {
            path: socket_path.clone(),
            owner,
            identity: Some(identity),
        };
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(SOCKET_MODE))?;
        validate_owned_socket(&socket_path, owner, &identity)?;
        socket_guard.identity = None;
        Ok(Self {
            socket,
            socket_path,
            socket_identity: identity,
            socket_directory,
            socket_directory_identity,
            owner,
            workers: Vec::with_capacity(ECU_IDS.len()),
            owns_workers,
            last_cycle_id: None,
        })
    }

    /// Generate a boot-monotonic wire cycle ID. It is independent from the runtime's 10 ms tick,
    /// which starts at zero when the vehicle application starts. A worker that outlives a vehicle
    /// application restart therefore still receives a strictly increasing ID for the same boot.
    pub fn next_cycle_id(&mut self) -> Result<u64, Error> {
        let now = monotonic_time_ns().map_err(Error::MonotonicClock)?;
        let cycle_id = match self.last_cycle_id {
            Some(previous) => now.max(previous.checked_add(1).ok_or(Error::CycleIdExhausted)?),
            None => now,
        };
        self.last_cycle_id = Some(cycle_id);
        Ok(cycle_id)
    }

    /// Send all ECU inputs first, then collect and validate the matching response from each
    /// worker before returning any output to the caller.
    pub fn exchange_cycle(
        &mut self,
        cycle_id: u64,
        inputs: [&[Frame]; 3],
        deadline: Instant,
    ) -> Result<[EcuTaskOutput; 3], Error> {
        let mut request_buffers = [[0; WORKER_DATAGRAM_MAX_SIZE]; 3];
        let mut request_lengths = [0; 3];
        for index in 0..ECU_IDS.len() {
            self.validate_coordinator_socket()?;
            self.check_worker_alive(index)?;
            request_lengths[index] = encode_request(
                ECU_IDS[index],
                cycle_id,
                inputs[index],
                &mut request_buffers[index],
            )?;
        }

        for index in 0..ECU_IDS.len() {
            if Instant::now() >= deadline {
                return Err(Error::DeadlineExceeded);
            }
            self.validate_coordinator_socket()?;
            self.check_worker_alive(index)?;
            let sent = self.socket.send_to(
                &request_buffers[index][..request_lengths[index]],
                &self.workers[index].socket_path,
            )?;
            if sent != request_lengths[index] {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short Unix datagram request write",
                )));
            }
        }

        let mut outputs: [Option<EcuTaskOutput>; 3] = [None; 3];
        let mut responses_received = 0;
        let mut response_buffer = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
        while responses_received < ECU_IDS.len() {
            self.validate_coordinator_socket()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or(Error::DeadlineExceeded)?;
            self.socket.set_read_timeout(Some(remaining))?;
            let (received, peer) = match self.socket.recv_from(&mut response_buffer) {
                Ok(response) => response,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(Error::DeadlineExceeded)
                }
                Err(error) => return Err(Error::Io(error)),
            };
            if received > WORKER_DATAGRAM_MAX_SIZE {
                return Err(Error::WorkerProtocol(WorkerError::InvalidLength));
            }
            let peer_path = peer.as_pathname().ok_or(Error::UnknownPeer)?;
            let worker_index = self
                .workers
                .iter()
                .position(|worker| worker.socket_path == peer_path)
                .ok_or(Error::UnknownPeer)?;
            self.check_worker_alive(worker_index)?;
            let response = decode_response(&response_buffer[..received])?;
            if response.ecu_id != self.workers[worker_index].ecu {
                return Err(Error::WrongWorkerEcu);
            }
            if response.cycle_id != cycle_id {
                return Err(Error::WrongCycleId);
            }
            if outputs[worker_index].is_some() {
                return Err(Error::DuplicateResponse(response.ecu_id));
            }
            outputs[worker_index] = Some(EcuTaskOutput {
                command: response.command,
                alive: response.alive,
            });
            responses_received += 1;
            if Instant::now() >= deadline {
                return Err(Error::DeadlineExceeded);
            }
        }

        let [Some(engine), Some(brake), Some(steering)] = outputs else {
            let missing_index = outputs.iter().position(Option::is_none).unwrap_or(0);
            return Err(Error::MissingResponse(ECU_IDS[missing_index]));
        };
        Ok([engine, brake, steering])
    }

    fn wait_for_worker(&mut self, index: usize, deadline: Instant) -> Result<(), Error> {
        let worker = &mut self.workers[index];
        loop {
            match fs::symlink_metadata(&worker.socket_path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_socket() || metadata.uid() != self.owner {
                        return Err(Error::InvalidWorkerSocket(worker.ecu));
                    }
                    worker.socket_identity.get_or_insert(SocketIdentity {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    });
                    if let Some(status) = worker
                        .child
                        .as_mut()
                        .map(Child::try_wait)
                        .transpose()?
                        .flatten()
                    {
                        return Err(Error::WorkerExited {
                            ecu: worker.ecu,
                            status,
                        });
                    }
                    if metadata.permissions().mode() & 0o777 == SOCKET_MODE {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if let Some(status) = worker
                        .child
                        .as_mut()
                        .map(Child::try_wait)
                        .transpose()?
                        .flatten()
                    {
                        return Err(Error::WorkerExited {
                            ecu: worker.ecu,
                            status,
                        });
                    }
                }
                Err(error) => return Err(Error::Io(error)),
            }
            if Instant::now() >= deadline {
                return Err(Error::WorkerStartupTimeout(worker.ecu));
            }
            thread::sleep(WORKER_STARTUP_POLL_INTERVAL);
        }
    }

    fn wait_for_external_worker(&mut self, index: usize, deadline: Instant) -> Result<(), Error> {
        let worker = &mut self.workers[index];
        loop {
            match fs::symlink_metadata(&worker.socket_path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_socket() || metadata.uid() != self.owner {
                        return Err(Error::InvalidWorkerSocket(worker.ecu));
                    }
                    let identity = SocketIdentity {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    };
                    if let Some(previous) = worker.socket_identity {
                        if previous.device != identity.device || previous.inode != identity.inode {
                            return Err(Error::InvalidWorkerSocket(worker.ecu));
                        }
                    } else {
                        worker.socket_identity = Some(identity);
                    }
                    if metadata.permissions().mode() & 0o777 == SOCKET_MODE {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::Io(error)),
            }
            if Instant::now() >= deadline {
                return Err(Error::WorkerStartupTimeout(worker.ecu));
            }
            thread::sleep(WORKER_STARTUP_POLL_INTERVAL);
        }
    }

    fn validate_coordinator_socket(&self) -> Result<(), Error> {
        validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .map_err(|_| Error::UnsafeSocketPath)?;
        validate_owned_socket(&self.socket_path, self.owner, &self.socket_identity)
            .map_err(|_| Error::UnsafeSocketPath)
    }

    fn check_worker_alive(&mut self, index: usize) -> Result<(), Error> {
        validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .map_err(|_| Error::UnsafeSocketPath)?;
        if let Some(child) = self.workers[index].child.as_mut() {
            if let Some(status) = child.try_wait()? {
                return Err(Error::WorkerExited {
                    ecu: self.workers[index].ecu,
                    status,
                });
            }
        }
        let identity = self.workers[index]
            .socket_identity
            .as_ref()
            .ok_or(Error::InvalidWorkerSocket(self.workers[index].ecu))?;
        validate_owned_socket(&self.workers[index].socket_path, self.owner, identity)
            .map_err(|_| Error::InvalidWorkerSocket(self.workers[index].ecu))
    }
}

impl Drop for EcuProcessCoordinator {
    fn drop(&mut self) {
        if self.owns_workers {
            for worker in &mut self.workers {
                if let Some(child) = worker.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        if validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .is_ok()
        {
            if self.owns_workers {
                for worker in &self.workers {
                    if let Some(identity) = &worker.socket_identity {
                        remove_socket_if_same(&worker.socket_path, self.owner, identity);
                    }
                }
            }
            remove_socket_if_same(&self.socket_path, self.owner, &self.socket_identity);
        }
    }
}

fn ecu_name(ecu: VirtualEcuId) -> &'static str {
    match ecu {
        VirtualEcuId::Engine => "engine",
        VirtualEcuId::Brake => "brake",
        VirtualEcuId::Steering => "steering",
    }
}

fn ensure_path_absent(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(Error::SocketPathInUse(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

fn prepare_socket_directory(path: &Path, owner: u32) -> Result<(), Error> {
    if !safe_absolute_path(path) {
        return Err(Error::UnsafeSocketPath);
    }
    let parent = path.parent().ok_or(Error::UnsafeSocketPath)?;
    validate_directory_chain(parent, owner)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir()
                || metadata.uid() != owner
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(Error::UnsafeSocketPath);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))?;
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.file_type().is_dir()
                || metadata.uid() != owner
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(Error::UnsafeSocketPath);
            }
        }
        Err(error) => return Err(Error::Io(error)),
    }
    Ok(())
}

fn validate_existing_socket_directory(path: &Path, owner: u32) -> Result<SocketIdentity, Error> {
    if !safe_absolute_path(path) {
        return Err(Error::UnsafeSocketPath);
    }
    let parent = path.parent().ok_or(Error::UnsafeSocketPath)?;
    validate_directory_chain(parent, owner)?;
    directory_identity(path, owner)
}

fn validate_worker_executable(path: &Path, owner: u32) -> Result<(), Error> {
    if !safe_absolute_path(path) {
        return Err(Error::UnsafeWorkerExecutable(path.to_path_buf()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafeWorkerExecutable(path.to_path_buf()))?;
    validate_trusted_directory_chain(parent, owner)
        .map_err(|_| Error::UnsafeWorkerExecutable(path.to_path_buf()))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| Error::UnsafeWorkerExecutable(path.to_path_buf()))?;
    let mode = metadata.permissions().mode();
    if !metadata.file_type().is_file()
        || (metadata.uid() != owner && metadata.uid() != 0)
        || mode & 0o022 != 0
        || mode & 0o111 == 0
    {
        return Err(Error::UnsafeWorkerExecutable(path.to_path_buf()));
    }
    Ok(())
}

fn validate_trusted_directory_chain(path: &Path, owner: u32) -> Result<(), Error> {
    if !safe_absolute_path(path) {
        return Err(Error::UnsafeWorkerExecutable(path.to_path_buf()));
    }
    let mut current = path;
    loop {
        let metadata = fs::symlink_metadata(current)
            .map_err(|_| Error::UnsafeWorkerExecutable(path.to_path_buf()))?;
        let mode = metadata.permissions().mode();
        let trusted_owner = metadata.uid() == owner || metadata.uid() == 0;
        let sticky_root_directory = metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.file_type().is_dir()
            || !trusted_owner
            || (mode & 0o022 != 0 && !sticky_root_directory)
        {
            return Err(Error::UnsafeWorkerExecutable(path.to_path_buf()));
        }
        if current == Path::new("/") {
            break;
        }
        current = current
            .parent()
            .ok_or_else(|| Error::UnsafeWorkerExecutable(path.to_path_buf()))?;
    }
    Ok(())
}

fn validate_directory_chain(path: &Path, owner: u32) -> Result<(), Error> {
    if !safe_absolute_path(path) {
        return Err(Error::UnsafeSocketPath);
    }
    let mut current = path;
    let mut immediate = true;
    loop {
        let metadata = fs::symlink_metadata(current)?;
        let mode = metadata.permissions().mode();
        let trusted_owner = metadata.uid() == owner || metadata.uid() == 0;
        let sticky_root_directory = metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.file_type().is_dir()
            || !trusted_owner
            || (immediate && metadata.uid() != owner)
            || (mode & 0o022 != 0 && !sticky_root_directory)
        {
            return Err(Error::UnsafeSocketPath);
        }
        if current == Path::new("/") {
            break;
        }
        current = current.parent().ok_or(Error::UnsafeSocketPath)?;
        immediate = false;
    }
    Ok(())
}

fn safe_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn socket_identity(path: &Path, owner: u32) -> Result<SocketIdentity, Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != owner {
        return Err(Error::UnsafeSocketPath);
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn directory_identity(path: &Path, owner: u32) -> Result<SocketIdentity, Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(Error::UnsafeSocketPath);
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn validate_private_directory(
    path: &Path,
    owner: u32,
    identity: &SocketIdentity,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.dev() != identity.device
        || metadata.ino() != identity.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "worker socket directory identity or permissions changed",
        ));
    }
    Ok(())
}

fn validate_owned_socket(path: &Path, owner: u32, identity: &SocketIdentity) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o777 != SOCKET_MODE
        || metadata.dev() != identity.device
        || metadata.ino() != identity.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Unix socket identity or permissions changed",
        ));
    }
    Ok(())
}

fn remove_socket_if_same(path: &Path, owner: u32, identity: &SocketIdentity) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket()
            && metadata.uid() == owner
            && metadata.dev() == identity.device
            && metadata.ino() == identity.inode
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn monotonic_time_ns() -> io::Result<u64> {
    let mut now = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `clock_gettime` initializes the provided timespec when it succeeds.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, now.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call above initialized `now`.
    let now = unsafe { now.assume_init() };
    let seconds = u64::try_from(now.tv_sec)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative monotonic time"))?;
    let nanoseconds = u64::try_from(now.tv_nsec)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid monotonic time"))?;
    seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|value| value.checked_add(nanoseconds))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "monotonic time overflow"))
}
