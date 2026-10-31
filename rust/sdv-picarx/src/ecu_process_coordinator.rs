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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
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
const MAX_CANDIDATE_DATAGRAMS_PER_CYCLE: usize = 3;
const CANDIDATE_RESPONSE_TIMEOUT_CYCLES: u8 = 2;
const CANDIDATE_READY_CONSECUTIVE_CYCLES: u16 = 100;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const COORDINATOR_SOCKET_NAME: &str = "coordinator.sock";
const WORKER_SOCKET_NAMES: [&str; 3] = ["engine.sock", "brake.sock", "steering.sock"];
const CANDIDATE_SOCKET_NAMES: [&str; 3] = ["engine.sock", "brake.sock", "steering.sock"];
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

/// Candidate-side validation result. Candidate failures never change active worker results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateFailure {
    UnsafeSocket,
    Send,
    Protocol,
    WrongEcu,
    WrongCycle,
    DuplicateResponse,
    Timeout,
}

/// Shadow-candidate state for one ECU. `Ready` means 100 consecutive host-cycle requests received
/// matching responses. Candidate output is always discarded and is never actuator input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateStatus {
    Disabled,
    Unavailable,
    WarmingUp { consecutive_responses: u16 },
    Ready,
    Failed(CandidateFailure),
}

impl CandidateStatus {
    fn after_window_reset(self) -> Self {
        match self {
            Self::Disabled | Self::Unavailable | Self::Failed(_) => self,
            Self::WarmingUp { .. } | Self::Ready => Self::WarmingUp {
                consecutive_responses: 0,
            },
        }
    }
}

fn status_after_queue_drop(
    statuses: [CandidateStatus; 3],
    queue_drop_generation: usize,
    applied_queue_drop_generation: usize,
) -> [CandidateStatus; 3] {
    if queue_drop_generation != applied_queue_drop_generation {
        statuses.map(CandidateStatus::after_window_reset)
    } else {
        statuses
    }
}

fn record_candidate_queue_drop(dropped_cycles: &AtomicUsize, drop_generation: &AtomicUsize) {
    let _ = dropped_cycles.try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(1))
    });
    drop_generation.fetch_add(1, Ordering::AcqRel);
}

#[derive(Default)]
struct CandidateStability {
    consecutive_responses: u16,
    last_success_host_cycle_sequence: Option<u64>,
    last_success_cycle_id: Option<u64>,
}

impl CandidateStability {
    fn reset_window(&mut self) {
        self.consecutive_responses = 0;
        self.last_success_host_cycle_sequence = None;
    }

    fn status(&self) -> CandidateStatus {
        if self.consecutive_responses >= CANDIDATE_READY_CONSECUTIVE_CYCLES {
            CandidateStatus::Ready
        } else {
            CandidateStatus::WarmingUp {
                consecutive_responses: self.consecutive_responses,
            }
        }
    }

    fn observe_success(&mut self, host_cycle_sequence: u64, cycle_id: u64) -> CandidateStatus {
        let follows_host_cycle = self
            .last_success_host_cycle_sequence
            .map_or(true, |previous| {
                previous.checked_add(1) == Some(host_cycle_sequence)
            });
        let has_increasing_cycle_id = self
            .last_success_cycle_id
            .map_or(true, |previous| cycle_id > previous);
        if !follows_host_cycle || !has_increasing_cycle_id {
            self.reset_window();
        }
        self.consecutive_responses = self
            .consecutive_responses
            .saturating_add(1)
            .min(CANDIDATE_READY_CONSECUTIVE_CYCLES);
        self.last_success_host_cycle_sequence = Some(host_cycle_sequence);
        self.last_success_cycle_id = Some(cycle_id);
        if self.consecutive_responses == CANDIDATE_READY_CONSECUTIVE_CYCLES {
            CandidateStatus::Ready
        } else {
            CandidateStatus::WarmingUp {
                consecutive_responses: self.consecutive_responses,
            }
        }
    }
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

#[derive(Clone, Copy, PartialEq, Eq)]
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

struct CandidateWorkerProcess {
    ecu: VirtualEcuId,
    socket_path: PathBuf,
    socket_identity: Option<SocketIdentity>,
}

struct CandidateCycleSnapshot {
    host_cycle_sequence: u64,
    cycle_id: u64,
    deadline: Instant,
    request_buffers: [[u8; WORKER_DATAGRAM_MAX_SIZE]; 3],
    request_lengths: [usize; 3],
}

struct CandidateShadowWorker {
    socket: UnixDatagram,
    _socket_path_guard: SocketPathGuard,
    socket_path: PathBuf,
    socket_identity: SocketIdentity,
    socket_directory: PathBuf,
    socket_directory_identity: SocketIdentity,
    owner: u32,
    workers: Vec<CandidateWorkerProcess>,
    statuses: [CandidateStatus; 3],
    stability: [CandidateStability; 3],
    shared_statuses: Arc<Mutex<[CandidateStatus; 3]>>,
    shared_last_response_cycle: Arc<Mutex<[Option<u64>; 3]>>,
    queue_drop_generation: Arc<AtomicUsize>,
    applied_queue_drop_generation: Arc<AtomicUsize>,
    pending_cycle: [Option<u64>; 3],
    pending_host_cycle_sequence: [Option<u64>; 3],
    pending_age: [u8; 3],
    last_request_host_cycle_sequence: [Option<u64>; 3],
    discard_responses_through_cycle: [Option<u64>; 3],
    last_processed_host_cycle_sequence: Option<u64>,
    processed_queue_drop_generation: usize,
    last_response_cycle: [Option<u64>; 3],
    last_logged_warming_progress: [u16; 3],
    dropped_cycles: Arc<AtomicUsize>,
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
    candidate_sender: Option<SyncSender<CandidateCycleSnapshot>>,
    candidate_dropped_cycles: Option<Arc<AtomicUsize>>,
    candidate_queue_drop_generation: Arc<AtomicUsize>,
    candidate_applied_queue_drop_generation: Arc<AtomicUsize>,
    candidate_statuses: Arc<Mutex<[CandidateStatus; 3]>>,
    candidate_last_response_cycles: Arc<Mutex<[Option<u64>; 3]>>,
    candidate_thread: Option<JoinHandle<()>>,
    owns_workers: bool,
    last_cycle_id: Option<u64>,
    next_host_cycle_sequence: u64,
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
        Self::connect_external_inner(socket_directory.as_ref(), None)
    }

    /// Connect to active worker sockets and optionally shadow the same cycle to candidate worker
    /// sockets in a separate private directory. Candidate sockets are optional and are never part
    /// of active readiness. Candidate responses are validated and discarded; candidate faults do
    /// not fail the active ECU cycle.
    pub fn connect_external_with_candidates(
        socket_directory: impl AsRef<Path>,
        candidate_socket_directory: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        Self::connect_external_inner(
            socket_directory.as_ref(),
            Some(candidate_socket_directory.as_ref()),
        )
    }

    fn connect_external_inner(
        socket_directory: &Path,
        candidate_socket_directory: Option<&Path>,
    ) -> Result<Self, Error> {
        let owner = unsafe { libc::geteuid() };
        let socket_directory = socket_directory.to_path_buf();
        let socket_directory_identity =
            validate_existing_socket_directory(&socket_directory, owner)?;
        let mut coordinator = Self::bind_coordinator_socket(
            socket_directory,
            socket_directory_identity,
            owner,
            false,
        )?;
        if let Some(candidate_directory) = candidate_socket_directory {
            match start_candidate_shadow_worker(
                candidate_directory,
                &coordinator.socket_directory,
                owner,
                Arc::clone(&coordinator.candidate_statuses),
                Arc::clone(&coordinator.candidate_last_response_cycles),
                Arc::clone(&coordinator.candidate_queue_drop_generation),
                Arc::clone(&coordinator.candidate_applied_queue_drop_generation),
            ) {
                Ok((sender, dropped_cycles, worker)) => {
                    coordinator.candidate_sender = Some(sender);
                    coordinator.candidate_dropped_cycles = Some(dropped_cycles);
                    coordinator.candidate_thread = Some(worker);
                }
                Err(error) => {
                    if let Ok(mut statuses) = coordinator.candidate_statuses.lock() {
                        *statuses = [CandidateStatus::Failed(CandidateFailure::UnsafeSocket); 3];
                    }
                    // Candidate setup is optional. A candidate socket/configuration fault must
                    // not prevent the active coordinator from starting.
                    eprintln!("SDV shadow candidate subsystem unavailable: {error}");
                }
            }
        }
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
            candidate_sender: None,
            candidate_dropped_cycles: None,
            candidate_queue_drop_generation: Arc::new(AtomicUsize::new(0)),
            candidate_applied_queue_drop_generation: Arc::new(AtomicUsize::new(0)),
            candidate_statuses: Arc::new(Mutex::new([CandidateStatus::Disabled; 3])),
            candidate_last_response_cycles: Arc::new(Mutex::new([None; 3])),
            candidate_thread: None,
            owns_workers,
            last_cycle_id: None,
            next_host_cycle_sequence: 0,
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

    /// Return per-ECU shadow-candidate status in Engine, Brake, Steering order.
    pub fn candidate_statuses(&self) -> [CandidateStatus; 3] {
        if self
            .candidate_thread
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            return [CandidateStatus::Failed(CandidateFailure::Protocol); 3];
        }
        let (statuses, applied_queue_drop_generation) = match self.candidate_statuses.lock() {
            Ok(statuses) => (
                *statuses,
                self.candidate_applied_queue_drop_generation
                    .load(Ordering::Acquire),
            ),
            Err(_) => return [CandidateStatus::Failed(CandidateFailure::Protocol); 3],
        };
        let queue_drop_generation = self.candidate_queue_drop_generation.load(Ordering::Acquire);
        status_after_queue_drop(
            statuses,
            queue_drop_generation,
            applied_queue_drop_generation,
        )
    }

    /// Return the last candidate response cycle validated per ECU.
    ///
    /// This is a diagnostic watermark only. It does not make an outstanding candidate request
    /// ready for routing and is never used as active-worker or vehicle-health evidence.
    pub fn candidate_last_validated_cycles(&self) -> [Option<u64>; 3] {
        self.candidate_last_response_cycles
            .lock()
            .map(|cycles| *cycles)
            .unwrap_or([None; 3])
    }

    /// Send all ECU inputs first, then collect and validate the matching response from each
    /// worker before returning any output to the caller.
    pub fn exchange_cycle(
        &mut self,
        cycle_id: u64,
        inputs: [&[Frame]; 3],
        deadline: Instant,
    ) -> Result<[EcuTaskOutput; 3], Error> {
        let host_cycle_sequence = self.next_host_cycle_sequence;
        self.next_host_cycle_sequence = self
            .next_host_cycle_sequence
            .checked_add(1)
            .ok_or(Error::CycleIdExhausted)?;
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
            let peer_path = peer.as_pathname().ok_or(Error::UnknownPeer)?;
            if received > WORKER_DATAGRAM_MAX_SIZE {
                return Err(Error::WorkerProtocol(WorkerError::InvalidLength));
            }
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
        self.queue_candidate_cycle(
            host_cycle_sequence,
            cycle_id,
            deadline,
            request_buffers,
            request_lengths,
        );
        Ok([engine, brake, steering])
    }

    fn queue_candidate_cycle(
        &self,
        host_cycle_sequence: u64,
        cycle_id: u64,
        deadline: Instant,
        request_buffers: [[u8; WORKER_DATAGRAM_MAX_SIZE]; 3],
        request_lengths: [usize; 3],
    ) {
        let (Some(sender), Some(dropped_cycles)) = (
            self.candidate_sender.as_ref(),
            self.candidate_dropped_cycles.as_ref(),
        ) else {
            return;
        };
        let snapshot = CandidateCycleSnapshot {
            host_cycle_sequence,
            cycle_id,
            deadline,
            request_buffers,
            request_lengths,
        };
        if matches!(sender.try_send(snapshot), Err(TrySendError::Full(_))) {
            record_candidate_queue_drop(dropped_cycles, &self.candidate_queue_drop_generation);
        }
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
        self.candidate_sender.take();
        if let Some(worker) = self.candidate_thread.take() {
            let _ = worker.join();
        }
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

fn start_candidate_shadow_worker(
    candidate_directory: &Path,
    active_directory: &Path,
    owner: u32,
    shared_statuses: Arc<Mutex<[CandidateStatus; 3]>>,
    shared_last_response_cycle: Arc<Mutex<[Option<u64>; 3]>>,
    queue_drop_generation: Arc<AtomicUsize>,
    applied_queue_drop_generation: Arc<AtomicUsize>,
) -> Result<
    (
        SyncSender<CandidateCycleSnapshot>,
        Arc<AtomicUsize>,
        JoinHandle<()>,
    ),
    Error,
> {
    if candidate_directory == active_directory {
        return Err(Error::UnsafeSocketPath);
    }
    let socket_directory_identity = validate_existing_socket_directory(candidate_directory, owner)?;
    let socket_path = candidate_directory.join(COORDINATOR_SOCKET_NAME);
    ensure_path_absent(&socket_path)?;
    let socket = UnixDatagram::bind(&socket_path)?;
    let socket_identity = socket_identity(&socket_path, owner)?;
    let socket_path_guard = SocketPathGuard {
        path: socket_path.clone(),
        owner,
        identity: Some(socket_identity),
    };
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(SOCKET_MODE))?;
    validate_owned_socket(&socket_path, owner, &socket_identity)?;
    socket.set_nonblocking(true)?;

    let dropped_cycles = Arc::new(AtomicUsize::new(0));
    let worker = CandidateShadowWorker {
        socket,
        _socket_path_guard: socket_path_guard,
        socket_path,
        socket_identity,
        socket_directory: candidate_directory.to_path_buf(),
        socket_directory_identity,
        owner,
        workers: ECU_IDS
            .into_iter()
            .zip(CANDIDATE_SOCKET_NAMES)
            .map(|(ecu, socket_name)| CandidateWorkerProcess {
                ecu,
                socket_path: candidate_directory.join(socket_name),
                socket_identity: None,
            })
            .collect(),
        statuses: [CandidateStatus::Unavailable; 3],
        stability: std::array::from_fn(|_| CandidateStability::default()),
        shared_statuses: Arc::clone(&shared_statuses),
        shared_last_response_cycle: Arc::clone(&shared_last_response_cycle),
        processed_queue_drop_generation: queue_drop_generation.load(Ordering::Acquire),
        queue_drop_generation,
        applied_queue_drop_generation,
        pending_cycle: [None; 3],
        pending_host_cycle_sequence: [None; 3],
        pending_age: [0; 3],
        last_request_host_cycle_sequence: [None; 3],
        discard_responses_through_cycle: [None; 3],
        last_processed_host_cycle_sequence: None,
        last_response_cycle: [None; 3],
        last_logged_warming_progress: [0; 3],
        dropped_cycles: Arc::clone(&dropped_cycles),
    };
    if let Ok(mut statuses) = shared_statuses.lock() {
        *statuses = [CandidateStatus::Unavailable; 3];
    } else {
        return Err(Error::UnsafeSocketPath);
    }
    if let Ok(mut cycles) = shared_last_response_cycle.lock() {
        *cycles = [None; 3];
    } else {
        return Err(Error::UnsafeSocketPath);
    }
    let (sender, receiver) = sync_channel(1);
    let handle = thread::Builder::new()
        .name("sdv-shadow-ecu-worker".to_owned())
        .spawn(move || worker.run(receiver))?;
    Ok((sender, dropped_cycles, handle))
}

impl CandidateShadowWorker {
    fn run(mut self, receiver: Receiver<CandidateCycleSnapshot>) {
        while let Ok(snapshot) = receiver.recv() {
            self.process_snapshot(snapshot);
        }
    }

    fn process_snapshot(&mut self, snapshot: CandidateCycleSnapshot) {
        let previous_statuses = self.statuses;
        let dropped_cycles = self.dropped_cycles.swap(0, Ordering::Relaxed) != 0;
        let queue_drop_generation = self.queue_drop_generation.load(Ordering::Acquire);
        let consecutive_snapshot = self
            .last_processed_host_cycle_sequence
            .map_or(true, |previous| {
                previous.checked_add(1) == Some(snapshot.host_cycle_sequence)
            });
        let pending_cycle_gap = self.pending_host_cycle_sequence.iter().any(|pending| {
            pending.is_some_and(|pending| {
                pending
                    .checked_add(1)
                    .is_some_and(|next| snapshot.host_cycle_sequence > next)
            })
        });
        let expired = Instant::now() >= snapshot.deadline;
        let reset_window = dropped_cycles
            || queue_drop_generation != self.processed_queue_drop_generation
            || !consecutive_snapshot
            || pending_cycle_gap
            || expired;
        if reset_window {
            self.reset_all_candidate_windows();
            self.processed_queue_drop_generation = queue_drop_generation;
        }
        self.last_processed_host_cycle_sequence = Some(snapshot.host_cycle_sequence);
        self.drain_candidate_responses();
        if !expired && Instant::now() < snapshot.deadline {
            self.send_candidate_requests(&snapshot);
            self.advance_candidate_timeouts();
        }
        if Instant::now() >= snapshot.deadline {
            self.reset_all_candidate_windows();
        }
        // A queue-full can race response processing. Reset again before publishing so a response
        // processed from the old window can never remain visible after the drop notification.
        let final_queue_drop_generation = self.queue_drop_generation.load(Ordering::Acquire);
        if final_queue_drop_generation != self.processed_queue_drop_generation {
            self.reset_all_candidate_windows();
            self.processed_queue_drop_generation = final_queue_drop_generation;
        }
        self.log_candidate_status_changes(previous_statuses);
        if let Ok(mut statuses) = self.shared_statuses.lock() {
            *statuses = self.statuses;
            // Publish the status snapshot and its applied queue-drop generation together. Readers
            // take this mutex before loading the generation, so they cannot pair an old Ready
            // snapshot with a newly applied generation and miss the immediate invalidation.
            self.applied_queue_drop_generation
                .store(self.processed_queue_drop_generation, Ordering::Release);
        }
        if let Ok(mut cycles) = self.shared_last_response_cycle.lock() {
            *cycles = self.last_response_cycle;
        }
    }

    fn send_candidate_requests(&mut self, snapshot: &CandidateCycleSnapshot) {
        if validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .is_err()
            || !self.validate_coordinator_socket()
        {
            self.fail_all_candidates(CandidateFailure::UnsafeSocket);
            return;
        }

        for index in 0..self.workers.len() {
            if self.pending_cycle[index].is_some() {
                if matches!(self.statuses[index], CandidateStatus::Failed(_)) {
                    self.reset_candidate_window(index);
                } else {
                    continue;
                }
            }
            if self.last_request_host_cycle_sequence[index].is_some_and(|previous| {
                previous.checked_add(1) != Some(snapshot.host_cycle_sequence)
            }) {
                self.reset_candidate_window(index);
            }
            let metadata = match fs::symlink_metadata(&self.workers[index].socket_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.reset_candidate_window(index);
                    self.workers[index].socket_identity = None;
                    self.statuses[index] = CandidateStatus::Unavailable;
                    continue;
                }
                Err(_) => {
                    self.fail_candidate(index, CandidateFailure::UnsafeSocket);
                    continue;
                }
            };
            if !metadata.file_type().is_socket()
                || metadata.uid() != self.owner
                || metadata.permissions().mode() & 0o777 != SOCKET_MODE
            {
                self.workers[index].socket_identity = None;
                self.fail_candidate(index, CandidateFailure::UnsafeSocket);
                continue;
            }
            let current_identity = SocketIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            };
            let previous_identity = self.workers[index].socket_identity;
            if matches!(self.statuses[index], CandidateStatus::Failed(_))
                && previous_identity.is_some_and(|previous| {
                    previous.device == current_identity.device
                        && previous.inode == current_identity.inode
                })
            {
                continue;
            }
            if matches!(self.statuses[index], CandidateStatus::Failed(_)) {
                self.reset_candidate_window(index);
                self.statuses[index] = CandidateStatus::Unavailable;
            } else if previous_identity.is_some_and(|previous| {
                previous.device != current_identity.device
                    || previous.inode != current_identity.inode
            }) {
                self.reset_candidate_window(index);
            }
            self.workers[index].socket_identity = Some(current_identity);
            match self.socket.send_to(
                &snapshot.request_buffers[index][..snapshot.request_lengths[index]],
                &self.workers[index].socket_path,
            ) {
                Ok(sent) if sent == snapshot.request_lengths[index] => {
                    self.pending_cycle[index] = Some(snapshot.cycle_id);
                    self.pending_host_cycle_sequence[index] = Some(snapshot.host_cycle_sequence);
                    self.pending_age[index] = 0;
                    self.last_request_host_cycle_sequence[index] =
                        Some(snapshot.host_cycle_sequence);
                    if !matches!(self.statuses[index], CandidateStatus::Failed(_)) {
                        self.statuses[index] = self.stability[index].status();
                    }
                }
                _ => self.fail_candidate(index, CandidateFailure::Send),
            }
        }
    }

    fn drain_candidate_responses(&mut self) {
        if validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .is_err()
            || !self.validate_coordinator_socket()
        {
            self.fail_all_candidates(CandidateFailure::UnsafeSocket);
            return;
        }
        let mut response_buffer = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
        for _ in 0..MAX_CANDIDATE_DATAGRAMS_PER_CYCLE {
            let (received, peer) = match self.socket.recv_from(&mut response_buffer) {
                Ok(response) => response,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.fail_all_candidates(CandidateFailure::UnsafeSocket);
                    break;
                }
            };
            let Some(peer_path) = peer.as_pathname() else {
                continue;
            };
            let Some(index) = self
                .workers
                .iter()
                .position(|worker| worker.socket_path == peer_path)
            else {
                continue;
            };
            if matches!(self.statuses[index], CandidateStatus::Failed(_)) {
                continue;
            }
            if !self.candidate_worker_identity_is_current(index) {
                self.fail_candidate(index, CandidateFailure::UnsafeSocket);
                continue;
            }
            if received > WORKER_DATAGRAM_MAX_SIZE {
                self.fail_candidate(index, CandidateFailure::Protocol);
                continue;
            }
            let response = match decode_response(&response_buffer[..received]) {
                Ok(response) => response,
                Err(_) => {
                    self.fail_candidate(index, CandidateFailure::Protocol);
                    continue;
                }
            };
            if self.discard_responses_through_cycle[index]
                .is_some_and(|cycle_id| response.cycle_id <= cycle_id)
            {
                continue;
            }
            let Some(expected_cycle) = self.pending_cycle[index] else {
                let failure = if self.last_response_cycle[index] == Some(response.cycle_id) {
                    CandidateFailure::DuplicateResponse
                } else {
                    CandidateFailure::Protocol
                };
                self.fail_candidate(index, failure);
                continue;
            };
            if response.ecu_id != self.workers[index].ecu {
                self.fail_candidate(index, CandidateFailure::WrongEcu);
                continue;
            }
            if response.cycle_id != expected_cycle {
                self.fail_candidate(index, CandidateFailure::WrongCycle);
                continue;
            }
            let Some(host_cycle_sequence) = self.pending_host_cycle_sequence[index] else {
                self.fail_candidate(index, CandidateFailure::Protocol);
                continue;
            };
            if self.stability[index]
                .last_success_cycle_id
                .is_some_and(|last_cycle| expected_cycle <= last_cycle)
            {
                self.fail_candidate(index, CandidateFailure::WrongCycle);
                continue;
            }
            self.pending_cycle[index] = None;
            self.pending_host_cycle_sequence[index] = None;
            self.pending_age[index] = 0;
            self.last_response_cycle[index] = Some(expected_cycle);
            self.discard_responses_through_cycle[index] = None;
            // Candidate output is deliberately discarded. Only 100 consecutive matching
            // responses can make the protocol-level shadow status Ready.
            self.statuses[index] =
                self.stability[index].observe_success(host_cycle_sequence, expected_cycle);
        }
    }

    fn advance_candidate_timeouts(&mut self) {
        for index in 0..self.pending_cycle.len() {
            if self.pending_cycle[index].is_none()
                || matches!(self.statuses[index], CandidateStatus::Failed(_))
            {
                continue;
            }
            self.pending_age[index] = self.pending_age[index].saturating_add(1);
            if self.pending_age[index] >= CANDIDATE_RESPONSE_TIMEOUT_CYCLES {
                self.fail_candidate(index, CandidateFailure::Timeout);
            }
        }
    }

    fn fail_all_candidates(&mut self, failure: CandidateFailure) {
        for index in 0..self.statuses.len() {
            self.fail_candidate(index, failure);
        }
    }

    fn reset_candidate_window(&mut self, index: usize) {
        let stale_cycle = self.pending_cycle[index]
            .into_iter()
            .chain(self.last_response_cycle[index])
            .max();
        if let Some(stale_cycle) = stale_cycle {
            self.discard_responses_through_cycle[index] = Some(
                self.discard_responses_through_cycle[index]
                    .map_or(stale_cycle, |previous| previous.max(stale_cycle)),
            );
        }
        self.pending_cycle[index] = None;
        self.pending_host_cycle_sequence[index] = None;
        self.pending_age[index] = 0;
        self.last_request_host_cycle_sequence[index] = None;
        self.stability[index].reset_window();
        if matches!(
            self.statuses[index],
            CandidateStatus::Ready | CandidateStatus::WarmingUp { .. }
        ) {
            self.statuses[index] = CandidateStatus::WarmingUp {
                consecutive_responses: 0,
            };
        }
    }

    fn reset_all_candidate_windows(&mut self) {
        for index in 0..self.statuses.len() {
            self.reset_candidate_window(index);
        }
    }

    fn fail_candidate(&mut self, index: usize, failure: CandidateFailure) {
        self.reset_candidate_window(index);
        self.statuses[index] = CandidateStatus::Failed(failure);
    }

    fn log_candidate_status_changes(&mut self, previous: [CandidateStatus; 3]) {
        for index in 0..self.statuses.len() {
            match self.statuses[index] {
                CandidateStatus::WarmingUp {
                    consecutive_responses,
                } => {
                    let should_log = if consecutive_responses == 0 {
                        previous[index] != self.statuses[index]
                    } else {
                        consecutive_responses == 1
                            || consecutive_responses % 10 == 0
                            || consecutive_responses == CANDIDATE_READY_CONSECUTIVE_CYCLES
                    };
                    if should_log
                        && (self.last_logged_warming_progress[index] != consecutive_responses
                            || previous[index] != self.statuses[index])
                    {
                        eprintln!(
                            "SDV shadow candidate {:?} status: WarmingUp ({}/{})",
                            ECU_IDS[index],
                            consecutive_responses,
                            CANDIDATE_READY_CONSECUTIVE_CYCLES
                        );
                        self.last_logged_warming_progress[index] = consecutive_responses;
                    }
                }
                CandidateStatus::Ready => {
                    if previous[index] != CandidateStatus::Ready {
                        eprintln!(
                            "SDV shadow candidate {:?} status: Ready ({} consecutive responses)",
                            ECU_IDS[index], CANDIDATE_READY_CONSECUTIVE_CYCLES
                        );
                    }
                }
                CandidateStatus::Unavailable => self.last_logged_warming_progress[index] = 0,
                CandidateStatus::Failed(_) => {
                    if self.statuses[index] != previous[index] {
                        eprintln!(
                            "SDV shadow candidate {:?} status: {:?}",
                            ECU_IDS[index], self.statuses[index]
                        );
                    }
                }
                CandidateStatus::Disabled => {}
            }
        }
    }

    fn validate_coordinator_socket(&self) -> bool {
        validate_owned_socket(&self.socket_path, self.owner, &self.socket_identity).is_ok()
    }

    fn candidate_worker_identity_is_current(&self, index: usize) -> bool {
        let Some(expected_identity) = self.workers[index].socket_identity.as_ref() else {
            return false;
        };
        validate_private_directory(
            &self.socket_directory,
            self.owner,
            &self.socket_directory_identity,
        )
        .is_ok()
            && validate_owned_socket(
                &self.workers[index].socket_path,
                self.owner,
                expected_identity,
            )
            .is_ok()
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

#[cfg(test)]
mod candidate_stability_tests {
    use super::*;

    #[test]
    fn ready_requires_exactly_one_hundred_consecutive_responses() {
        let mut stability = CandidateStability::default();
        for sequence in 0..CANDIDATE_READY_CONSECUTIVE_CYCLES - 1 {
            assert_eq!(
                stability.observe_success(u64::from(sequence), u64::from(sequence) + 10),
                CandidateStatus::WarmingUp {
                    consecutive_responses: sequence + 1
                }
            );
        }
        assert_eq!(
            stability.observe_success(99, 109),
            CandidateStatus::Ready,
            "the 100th consecutive response must be the first Ready state"
        );
        assert_eq!(
            stability.status(),
            CandidateStatus::Ready,
            "a new outstanding request must not demote stable candidate status"
        );
    }

    #[test]
    fn host_cycle_sequence_gap_restarts_the_consecutive_window() {
        let mut stability = CandidateStability::default();
        assert_eq!(
            stability.observe_success(20, 200),
            CandidateStatus::WarmingUp {
                consecutive_responses: 1
            }
        );
        assert_eq!(
            stability.observe_success(21, 210),
            CandidateStatus::WarmingUp {
                consecutive_responses: 2
            }
        );
        assert_eq!(
            stability.observe_success(23, 230),
            CandidateStatus::WarmingUp {
                consecutive_responses: 1
            }
        );
    }

    #[test]
    fn full_candidate_queue_immediately_masks_ready_status() {
        let (sender, _receiver) = sync_channel(1);
        let make_snapshot = || CandidateCycleSnapshot {
            host_cycle_sequence: 1,
            cycle_id: 100,
            deadline: Instant::now() + Duration::from_secs(1),
            request_buffers: [[0; WORKER_DATAGRAM_MAX_SIZE]; 3],
            request_lengths: [0; 3],
        };
        sender.try_send(make_snapshot()).unwrap();
        let dropped_cycles = AtomicUsize::new(0);
        let queue_drop_generation = AtomicUsize::new(0);
        assert!(matches!(
            sender.try_send(make_snapshot()),
            Err(TrySendError::Full(_))
        ));
        record_candidate_queue_drop(&dropped_cycles, &queue_drop_generation);

        assert_eq!(dropped_cycles.load(Ordering::Relaxed), 1);
        assert_eq!(
            status_after_queue_drop(
                [CandidateStatus::Ready; 3],
                queue_drop_generation.load(Ordering::Acquire),
                0,
            ),
            [CandidateStatus::WarmingUp {
                consecutive_responses: 0
            }; 3]
        );
    }

    #[test]
    fn expired_snapshot_and_candidate_failures_clear_ready_evidence() {
        let directory = std::env::temp_dir()
            .join(format!("sdv-candidate-expired-snapshot-{}", unsafe {
                libc::getpid()
            }));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(
            &directory,
            fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
        )
        .unwrap();
        let owner = unsafe { libc::geteuid() };
        let directory_identity = directory_identity(&directory, owner).unwrap();
        let socket_path = directory.join(COORDINATOR_SOCKET_NAME);
        let socket = UnixDatagram::bind(&socket_path).unwrap();
        socket.set_nonblocking(true).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(SOCKET_MODE)).unwrap();
        let socket_identity = socket_identity(&socket_path, owner).unwrap();
        let socket_guard = SocketPathGuard {
            path: socket_path.clone(),
            owner,
            identity: Some(socket_identity),
        };
        let shared_statuses = Arc::new(Mutex::new([
            CandidateStatus::Ready,
            CandidateStatus::Unavailable,
            CandidateStatus::Unavailable,
        ]));
        let shared_last_response_cycle = Arc::new(Mutex::new([Some(700), None, None]));
        let queue_drop_generation = Arc::new(AtomicUsize::new(0));
        let applied_queue_drop_generation = Arc::new(AtomicUsize::new(0));
        let dropped_cycles = Arc::new(AtomicUsize::new(0));
        let mut worker = CandidateShadowWorker {
            socket,
            _socket_path_guard: socket_guard,
            socket_path,
            socket_identity,
            socket_directory: directory.clone(),
            socket_directory_identity: directory_identity,
            owner,
            workers: ECU_IDS
                .into_iter()
                .zip(CANDIDATE_SOCKET_NAMES)
                .map(|(ecu, socket_name)| CandidateWorkerProcess {
                    ecu,
                    socket_path: directory.join(socket_name),
                    socket_identity: None,
                })
                .collect(),
            statuses: [
                CandidateStatus::Ready,
                CandidateStatus::Unavailable,
                CandidateStatus::Unavailable,
            ],
            stability: [
                CandidateStability {
                    consecutive_responses: CANDIDATE_READY_CONSECUTIVE_CYCLES,
                    last_success_host_cycle_sequence: Some(7),
                    last_success_cycle_id: Some(700),
                },
                CandidateStability::default(),
                CandidateStability::default(),
            ],
            shared_statuses: Arc::clone(&shared_statuses),
            shared_last_response_cycle: Arc::clone(&shared_last_response_cycle),
            queue_drop_generation,
            applied_queue_drop_generation,
            pending_cycle: [Some(800), None, None],
            pending_host_cycle_sequence: [Some(8), None, None],
            pending_age: [1, 0, 0],
            last_request_host_cycle_sequence: [Some(8), None, None],
            discard_responses_through_cycle: [None; 3],
            last_processed_host_cycle_sequence: Some(7),
            processed_queue_drop_generation: 0,
            last_response_cycle: [Some(700), None, None],
            last_logged_warming_progress: [0; 3],
            dropped_cycles,
        };

        worker.process_snapshot(CandidateCycleSnapshot {
            host_cycle_sequence: 8,
            cycle_id: 800,
            deadline: Instant::now() - Duration::from_millis(1),
            request_buffers: [[0; WORKER_DATAGRAM_MAX_SIZE]; 3],
            request_lengths: [0; 3],
        });

        assert_eq!(
            worker.statuses[0],
            CandidateStatus::WarmingUp {
                consecutive_responses: 0
            }
        );
        assert_eq!(worker.pending_cycle[0], None);
        assert_eq!(worker.stability[0].consecutive_responses, 0);
        assert_eq!(
            *shared_statuses.lock().unwrap(),
            [
                CandidateStatus::WarmingUp {
                    consecutive_responses: 0
                },
                CandidateStatus::Unavailable,
                CandidateStatus::Unavailable,
            ]
        );

        for failure in [
            CandidateFailure::Timeout,
            CandidateFailure::Protocol,
            CandidateFailure::WrongEcu,
            CandidateFailure::WrongCycle,
            CandidateFailure::UnsafeSocket,
            CandidateFailure::Send,
            CandidateFailure::DuplicateResponse,
        ] {
            worker.statuses[0] = CandidateStatus::Ready;
            worker.stability[0] = CandidateStability {
                consecutive_responses: CANDIDATE_READY_CONSECUTIVE_CYCLES,
                last_success_host_cycle_sequence: Some(20),
                last_success_cycle_id: Some(900),
            };
            worker.pending_cycle[0] = Some(910);
            worker.pending_host_cycle_sequence[0] = Some(21);
            worker.pending_age[0] = 1;
            worker.last_request_host_cycle_sequence[0] = Some(21);
            worker.fail_candidate(0, failure);

            assert_eq!(worker.statuses[0], CandidateStatus::Failed(failure));
            assert_eq!(worker.stability[0].consecutive_responses, 0);
            assert_eq!(worker.pending_cycle[0], None);
            assert_eq!(worker.pending_host_cycle_sequence[0], None);
        }
        drop(worker);
        fs::remove_dir_all(directory).unwrap();
    }
}
