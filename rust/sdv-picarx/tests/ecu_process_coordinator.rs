use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sdv_core::config::Config;
use sdv_core::model::VehicleState;
use sdv_core::platform::HostCycleExecutor;
use sdv_core::platform::{PlatformInputs, VehiclePlatform};
use sdv_core::rte::DriverInput;
use sdv_core::runtime::{ExternalEcuCycleError, VehicleActuation, VehicleRuntime, VirtualEcuId};
use sdv_core::runtime_host::{self, RunError, SystemClock};
use sdv_picarx::ecu_process_coordinator::{
    CandidateFailure, CandidateRouteOperation, CandidateRouteRequest, CandidateStatus,
    EcuProcessCoordinator, EcuProcessVehicleExecutor, EcuRoute, Error as CoordinatorError,
    PromotionError, VehicleCycleError,
};
use sdv_picarx::output_hold::StickyOutputHold;
use sdv_picarx::virtual_ecu_worker::{VirtualEcuWorker, WORKER_DATAGRAM_MAX_SIZE};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sdv-ecu-process-coordinator-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct RecordingPlatform {
    inputs: PlatformInputs,
    applied: Vec<VehicleActuation>,
    stops: usize,
}

impl VehiclePlatform for RecordingPlatform {
    type Error = &'static str;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        Ok(self.inputs)
    }

    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
        self.applied.push(actuation);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.stops += 1;
        Ok(())
    }
}

fn worker_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_sdv-picarx-ecu-worker"))
}

fn spawn_workers(root: &TempDirectory) -> EcuProcessCoordinator {
    EcuProcessCoordinator::spawn(worker_binary(), root.path().join("workers")).unwrap()
}

fn private_worker_directory(root: &TempDirectory) -> PathBuf {
    let path = root.path().join("workers");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

struct ExternalWorkerProcesses(Vec<Child>);

impl ExternalWorkerProcesses {
    fn start(socket_directory: &Path, coordinator_socket: &Path) -> Self {
        let children = ["engine", "brake", "steering"]
            .into_iter()
            .map(|ecu| {
                Command::new(worker_binary())
                    .arg("--ecu")
                    .arg(ecu)
                    .arg("--worker-socket")
                    .arg(socket_directory.join(format!("{ecu}.sock")))
                    .arg("--coordinator-socket")
                    .arg(coordinator_socket)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        Self(children)
    }

    fn stop(&mut self) {
        for child in &mut self.0 {
            // SAFETY: child.id() is the process ID returned by Command::spawn.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
        }
        for child in &mut self.0 {
            let _ = child.wait();
        }
    }

    fn start_one(ecu: &str, socket_directory: &Path, coordinator_socket: &Path) -> Self {
        Self::start_one_with_allowed(ecu, socket_directory, coordinator_socket, None)
    }

    fn start_one_with_allowed(
        ecu: &str,
        socket_directory: &Path,
        coordinator_socket: &Path,
        allowed_coordinator_socket: Option<&Path>,
    ) -> Self {
        let child = Command::new(worker_binary())
            .arg("--ecu")
            .arg(ecu)
            .arg("--worker-socket")
            .arg(socket_directory.join(format!("{ecu}.sock")))
            .arg("--coordinator-socket")
            .arg(coordinator_socket)
            .args(allowed_coordinator_socket.into_iter().flat_map(|path| {
                [
                    std::ffi::OsString::from("--allowed-coordinator-socket"),
                    path.as_os_str().to_owned(),
                ]
            }))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self(vec![child])
    }
}

impl Drop for ExternalWorkerProcesses {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

struct CandidateResponder {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CandidateResponder {
    fn start(path: &Path, fail_active_route: bool) -> Self {
        let socket = UnixDatagram::bind(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
            let mut request = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
            let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];
            while !thread_stop.load(Ordering::Relaxed) {
                let (received, coordinator) = match socket.recv_from(&mut request) {
                    Ok(received) => received,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue
                    }
                    Err(_) => break,
                };
                let Some(coordinator_path) = coordinator.as_pathname() else {
                    continue;
                };
                if fail_active_route && coordinator_path.to_string_lossy().contains("/workers/") {
                    let _ = socket.send_to(b"invalid-candidate-response\n", coordinator_path);
                    continue;
                }
                let response_len = match worker.handle_datagram(&request[..received], &mut response)
                {
                    Ok(response_len) => response_len,
                    Err(_) => continue,
                };
                let _ = socket.send_to(&response[..response_len], coordinator_path);
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for CandidateResponder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

struct HeldRecordingPlatform {
    inner: RecordingPlatform,
    hold: StickyOutputHold,
}

impl VehiclePlatform for HeldRecordingPlatform {
    type Error = &'static str;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        self.inner.sample()
    }

    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
        self.hold.apply(&mut self.inner, actuation)
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.inner.stop()
    }
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        path.exists(),
        "socket path did not appear: {}",
        path.display()
    );
}

fn start_external_coordinator(
    root: &TempDirectory,
    candidate_directory: &Path,
) -> (EcuProcessCoordinator, ExternalWorkerProcesses, PathBuf) {
    let socket_directory = private_worker_directory(root);
    let coordinator_path = socket_directory.join("coordinator.sock");
    let candidate_coordinator_path = candidate_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.to_path_buf();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    wait_for_path(&candidate_coordinator_path);
    let active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let coordinator = coordinator_thread.join().unwrap().unwrap();
    (coordinator, active, socket_directory)
}

fn candidate_inputs() -> [Vec<sdv_core::bus::Frame>; 3] {
    let safety = sdv_core::bus::Frame::Safety(Default::default());
    [vec![safety], vec![safety], vec![safety]]
}

fn run_candidate_until_ready(coordinator: &mut EcuProcessCoordinator) {
    let frames = candidate_inputs();
    let inputs = [
        frames[0].as_slice(),
        frames[1].as_slice(),
        frames[2].as_slice(),
    ];
    for _ in 0..102 {
        thread::sleep(Duration::from_millis(10));
        let cycle_id = coordinator.next_cycle_id().unwrap();
        coordinator
            .exchange_cycle(
                cycle_id,
                inputs,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
    }
    assert_eq!(
        wait_for_candidate_status(coordinator, |status| status == CandidateStatus::Ready),
        CandidateStatus::Ready
    );
}

fn confirmed_output_hold() -> StickyOutputHold {
    let hold = StickyOutputHold::new(false);
    assert!(!hold.request_and_wait(Duration::ZERO));
    let mut platform = RecordingPlatform::default();
    hold.apply(&mut platform, VehicleActuation::default())
        .unwrap();
    assert!(hold.is_held());
    assert!(hold.has_zero_apply_acknowledgement());
    hold
}

fn wait_for_candidate_status(
    coordinator: &EcuProcessCoordinator,
    mut matches: impl FnMut(CandidateStatus) -> bool,
) -> CandidateStatus {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let status = coordinator.candidate_statuses()[0];
        if matches(status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "candidate status timed out: {status:?}"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn startup_failure_reaps_prior_workers_and_removes_recorded_sockets() {
    let root = TempDirectory::new();
    let socket_directory = root.path().join("workers");
    let result =
        EcuProcessCoordinator::spawn(fake_worker_binary(&root, "startup-fail"), &socket_directory);
    assert!(
        matches!(&result, Err(CoordinatorError::WorkerStartupTimeout(_))),
        "{:?}",
        result.as_ref().err()
    );
    assert!(!socket_directory.join("engine.sock").exists());
    assert!(!socket_directory.join("brake.sock").exists());
    assert!(!socket_directory.join("coordinator.sock").exists());
}

#[test]
fn unsafe_worker_executable_symlink_and_permissions_are_rejected() {
    let root = TempDirectory::new();
    let writable = root.path().join("writable-worker");
    fs::copy(worker_binary(), &writable).unwrap();
    fs::set_permissions(&writable, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        EcuProcessCoordinator::spawn(&writable, root.path().join("workers")),
        Err(CoordinatorError::UnsafeWorkerExecutable(_))
    ));

    fs::remove_file(&writable).unwrap();
    symlink(worker_binary(), &writable).unwrap();
    assert!(matches!(
        EcuProcessCoordinator::spawn(&writable, root.path().join("workers")),
        Err(CoordinatorError::UnsafeWorkerExecutable(_))
    ));
    assert!(!root.path().join("workers").exists());
}

#[test]
fn three_worker_processes_match_in_process_vehicle_runtime_for_multiple_cycles() {
    let root = TempDirectory::new();
    let mut coordinator = spawn_workers(&root);
    let mut in_process = VehicleRuntime::default();
    let mut worker_runtime = VehicleRuntime::default();
    let mut previous_wire_id = None;

    for tick in 0..160_u64 {
        let input = DriverInput {
            throttle: if tick % 50 < 25 { 0.35 } else { 0.0 },
            brake: if tick % 50 < 25 { 0.0 } else { 0.15 },
            steer: if tick % 40 < 20 { 0.1 } else { -0.1 },
        };
        let state = VehicleState {
            t: tick as f32 * 0.01,
            v: input.throttle - input.brake,
            ..VehicleState::default()
        };
        in_process.set_estop_request(tick == 120);
        worker_runtime.set_estop_request(tick == 120);
        let expected = in_process.step_10ms(input, state, true);
        let cycle_deadline = Instant::now() + Duration::from_millis(100);
        let actual = worker_runtime
            .step_10ms_with_external_ecus(input, state, true, |runtime_tick, inputs| {
                assert_eq!(runtime_tick, tick);
                let wire_cycle = coordinator.next_cycle_id()?;
                if let Some(previous) = previous_wire_id {
                    assert!(wire_cycle > previous);
                }
                previous_wire_id = Some(wire_cycle);
                coordinator.exchange_cycle(wire_cycle, inputs, cycle_deadline)
            })
            .unwrap();
        assert_eq!(actual, expected, "vehicle output diverged at tick {tick}");
    }
}

#[test]
fn external_worker_services_are_ready_and_match_in_process_runtime() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        thread::spawn(move || EcuProcessCoordinator::connect_external(socket_directory))
    };
    wait_for_path(&coordinator_path);
    let mut processes = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();

    let mut in_process = VehicleRuntime::default();
    let mut worker_runtime = VehicleRuntime::default();
    let mut previous_wire_id = None;
    for tick in 0..80_u64 {
        let input = DriverInput {
            throttle: if tick % 40 < 20 { 0.3 } else { 0.0 },
            brake: if tick % 40 < 20 { 0.0 } else { 0.12 },
            steer: if tick % 32 < 16 { 0.08 } else { -0.08 },
        };
        let state = VehicleState {
            t: tick as f32 * 0.01,
            v: input.throttle - input.brake,
            ..VehicleState::default()
        };
        in_process.set_estop_request(tick == 60);
        worker_runtime.set_estop_request(tick == 60);
        let expected = in_process.step_10ms(input, state, true);
        let cycle_deadline = Instant::now() + Duration::from_millis(100);
        let actual = worker_runtime
            .step_10ms_with_external_ecus(input, state, true, |runtime_tick, inputs| {
                assert_eq!(runtime_tick, tick);
                let wire_cycle = coordinator.next_cycle_id()?;
                if let Some(previous) = previous_wire_id {
                    assert!(wire_cycle > previous);
                }
                previous_wire_id = Some(wire_cycle);
                coordinator.exchange_cycle(wire_cycle, inputs, cycle_deadline)
            })
            .unwrap();
        assert_eq!(actual, expected, "external output diverged at tick {tick}");
    }

    drop(coordinator);
    assert!(!coordinator_path.exists());
    for ecu in ["engine", "brake", "steering"] {
        assert!(socket_directory.join(format!("{ecu}.sock")).exists());
    }
    assert!(processes
        .0
        .iter_mut()
        .all(|child| child.try_wait().unwrap().is_none()));
    processes.stop();
    for ecu in ["engine", "brake", "steering"] {
        assert!(!socket_directory.join(format!("{ecu}.sock")).exists());
    }
}

#[test]
fn candidate_shadow_is_validated_but_never_changes_active_runtime_output() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    let candidate_coordinator_path = candidate_directory.join("coordinator.sock");
    wait_for_path(&candidate_coordinator_path);
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut candidate = ExternalWorkerProcesses::start_one(
        "engine",
        &candidate_directory,
        &candidate_coordinator_path,
    );
    wait_for_path(&candidate_directory.join("engine.sock"));
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    let mut in_process = VehicleRuntime::default();
    let mut worker_runtime = VehicleRuntime::default();

    for tick in 0..102_u64 {
        // Match the production 10 ms coordinator cadence so this test exercises a healthy
        // candidate worker instead of intentionally outrunning its process scheduler.
        thread::sleep(Duration::from_millis(10));
        let driver = DriverInput {
            throttle: 0.25,
            brake: 0.0,
            steer: 0.03,
        };
        let state = VehicleState {
            t: tick as f32 * 0.01,
            v: driver.throttle,
            ..VehicleState::default()
        };
        let expected = in_process.step_10ms(driver, state, true);
        let actual = worker_runtime
            .step_10ms_with_external_ecus(driver, state, true, |_, inputs| {
                let cycle_id = coordinator.next_cycle_id()?;
                coordinator.exchange_cycle(
                    cycle_id,
                    inputs,
                    Instant::now() + Duration::from_millis(100),
                )
            })
            .unwrap();
        assert_eq!(actual, expected);
        let expected_status = if tick < 100 {
            CandidateStatus::WarmingUp {
                consecutive_responses: tick as u16,
            }
        } else {
            CandidateStatus::Ready
        };
        assert_eq!(
            wait_for_candidate_status(&coordinator, |status| status == expected_status),
            expected_status,
            "candidate status at host cycle {tick}"
        );
    }

    assert!(candidate_directory.join("engine.sock").exists());
    drop(coordinator);
    candidate.stop();
    active.stop();
}

#[test]
fn candidate_timeout_does_not_fail_active_worker_cycle() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let candidate_socket = UnixDatagram::bind(&candidate_path).unwrap();
    fs::set_permissions(&candidate_path, fs::Permissions::from_mode(0o600)).unwrap();
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    wait_for_path(&candidate_directory.join("coordinator.sock"));
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    let engine_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let brake_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let steering_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let inputs = [
        engine_input.as_slice(),
        brake_input.as_slice(),
        steering_input.as_slice(),
    ];
    for _ in 0..6 {
        let cycle_id = coordinator.next_cycle_id().unwrap();
        let outputs = coordinator
            .exchange_cycle(
                cycle_id,
                inputs,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        assert!(outputs
            .iter()
            .all(|output| matches!(output.alive, sdv_core::bus::Frame::Alive { .. })));
    }
    assert_eq!(
        wait_for_candidate_status(&coordinator, |status| {
            matches!(status, CandidateStatus::Failed(_))
        }),
        CandidateStatus::Failed(CandidateFailure::Timeout)
    );
    drop(candidate_socket);
    drop(coordinator);
    active.stop();
}

#[test]
fn candidate_pending_request_clears_ready_and_times_out_after_two_cycles() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let candidate_socket = UnixDatagram::bind(&candidate_path).unwrap();
    fs::set_permissions(&candidate_path, fs::Permissions::from_mode(0o600)).unwrap();
    candidate_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (request_seen_tx, request_seen_rx) = std::sync::mpsc::channel();
    let candidate_responder = thread::spawn(move || {
        let mut request = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
        let (received, coordinator_addr) = candidate_socket.recv_from(&mut request).unwrap();
        let coordinator_path = coordinator_addr.as_pathname().unwrap();
        let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
        let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];
        let response_len = worker
            .handle_datagram(&request[..received], &mut response)
            .unwrap();
        candidate_socket
            .send_to(&response[..response_len], coordinator_path)
            .unwrap();
        request_seen_tx.send(1).unwrap();

        // Receive the following request but withhold its reply to model a candidate that stops
        // responding after one valid shadow cycle.
        let _ = candidate_socket.recv_from(&mut request).unwrap();
        request_seen_tx.send(2).unwrap();
    });
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    wait_for_path(&candidate_directory.join("coordinator.sock"));
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    let input = [sdv_core::bus::Frame::Safety(Default::default())];
    let inputs = [input.as_slice(), input.as_slice(), input.as_slice()];
    let mut first_candidate_cycle = None;

    for tick in 0..3 {
        thread::sleep(Duration::from_millis(10));
        let cycle_id = coordinator.next_cycle_id().unwrap();
        if tick == 0 {
            first_candidate_cycle = Some(cycle_id);
        }
        let outputs = coordinator
            .exchange_cycle(
                cycle_id,
                inputs,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        assert!(outputs
            .iter()
            .all(|output| matches!(output.alive, sdv_core::bus::Frame::Alive { .. })));
        if tick < 2 {
            request_seen_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            let expected_count = u16::from(tick == 1);
            assert_eq!(
                wait_for_candidate_status(&coordinator, |status| {
                    status
                        == CandidateStatus::WarmingUp {
                            consecutive_responses: expected_count,
                        }
                }),
                CandidateStatus::WarmingUp {
                    consecutive_responses: expected_count
                }
            );
            if tick == 1 {
                let first_candidate_cycle = first_candidate_cycle.unwrap();
                let deadline = Instant::now() + Duration::from_secs(1);
                while coordinator.candidate_last_validated_cycles()[0]
                    != Some(first_candidate_cycle)
                {
                    assert!(
                        Instant::now() < deadline,
                        "valid candidate cycle was not recorded"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
            }
        } else {
            assert_eq!(
                wait_for_candidate_status(&coordinator, |status| {
                    matches!(status, CandidateStatus::Failed(_))
                }),
                CandidateStatus::Failed(CandidateFailure::Timeout),
                "tick {tick}"
            );
            assert_eq!(
                coordinator.candidate_last_validated_cycles()[0],
                first_candidate_cycle
            );
        }
    }

    candidate_responder.join().unwrap();
    drop(coordinator);
    active.stop();
}

#[test]
fn promotion_requires_a_ready_candidate_and_confirmed_zero_output_hold() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_socket = UnixDatagram::bind(candidate_directory.join("engine.sock")).unwrap();
    fs::set_permissions(
        candidate_directory.join("engine.sock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let (mut coordinator, mut active, _) = start_external_coordinator(&root, &candidate_directory);

    let unconfirmed_hold = StickyOutputHold::new(false);
    assert_eq!(
        coordinator.begin_candidate_quiesce(VirtualEcuId::Engine, &unconfirmed_hold),
        Err(PromotionError::HoldNotConfirmed)
    );
    let confirmed_hold = confirmed_output_hold();
    assert_eq!(
        coordinator.begin_candidate_quiesce(VirtualEcuId::Engine, &confirmed_hold),
        Err(PromotionError::CandidateNotReady)
    );
    assert_eq!(
        coordinator.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active
    );
    assert!(confirmed_hold.is_held());

    drop(candidate_socket);
    drop(coordinator);
    active.stop();
}

#[test]
fn route_reversion_rejects_a_replaced_active_worker_socket() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let responder = CandidateResponder::start(&candidate_path, false);
    let (mut coordinator, mut active, active_directory) =
        start_external_coordinator(&root, &candidate_directory);
    run_candidate_until_ready(&mut coordinator);

    let hold = confirmed_output_hold();
    coordinator
        .begin_candidate_quiesce(VirtualEcuId::Engine, &hold)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match coordinator
            .finish_candidate_promotion(VirtualEcuId::Engine, &hold)
            .unwrap()
        {
            true => break,
            false => {
                assert!(
                    Instant::now() < deadline,
                    "candidate quiesce did not finish"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert_eq!(
        coordinator.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Candidate
    );

    // Keep the active worker alive while replacing its path, preventing inode reuse and matching
    // the live-service case. Reversion must reject the new endpoint and keep the candidate route.
    let active_path = active_directory.join("engine.sock");
    fs::remove_file(&active_path).unwrap();
    let replacement = UnixDatagram::bind(&active_path).unwrap();
    fs::set_permissions(&active_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        coordinator.return_candidate_route_to_active(VirtualEcuId::Engine, &hold),
        Err(PromotionError::ActiveWorkerUnavailable)
    );
    assert_eq!(
        coordinator.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Candidate
    );
    assert!(hold.is_held());

    drop(replacement);
    drop(coordinator);
    drop(responder);
    active.stop();
}

#[test]
fn expired_promotion_request_is_canceled_and_never_switches_route_later() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_socket = UnixDatagram::bind(candidate_directory.join("engine.sock")).unwrap();
    fs::set_permissions(
        candidate_directory.join("engine.sock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let (coordinator, mut active, _) = start_external_coordinator(&root, &candidate_directory);
    let hold = confirmed_output_hold();
    let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
    let (completion, response) = std::sync::mpsc::sync_channel(1);
    request_tx
        .send(CandidateRouteRequest {
            ecu: VirtualEcuId::Engine,
            operation: CandidateRouteOperation::Promote,
            deadline: Instant::now() - Duration::from_millis(1),
            completion,
        })
        .unwrap();
    let platform = HeldRecordingPlatform {
        inner: RecordingPlatform::default(),
        hold: hold.clone(),
    };
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator)
            .with_route_control(hold.clone(), request_rx);

    executor.host_cycle(Duration::from_millis(100)).unwrap();
    assert!(response
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .is_err());
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active
    );
    thread::sleep(Duration::from_millis(20));
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active,
        "an expired request must not switch asynchronously"
    );
    assert!(hold.is_held());

    drop(candidate_socket);
    drop(executor);
    active.stop();
}

#[test]
fn changed_candidate_socket_inode_is_rejected_before_route_switch() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let responder = CandidateResponder::start(&candidate_path, false);
    let (mut coordinator, mut active, _) = start_external_coordinator(&root, &candidate_directory);
    run_candidate_until_ready(&mut coordinator);
    // Keep the old socket bound while replacing its pathname so the filesystem cannot
    // immediately reuse its inode. This models a live candidate worker whose socket FD
    // remains open after an attacker or operator replaces the socket path.
    fs::remove_file(&candidate_path).unwrap();
    let replacement = CandidateResponder::start(&candidate_path, false);
    drop(responder);
    let hold = confirmed_output_hold();

    let promotion = coordinator.begin_candidate_quiesce(VirtualEcuId::Engine, &hold);
    assert!(
        matches!(
            promotion,
            Err(PromotionError::CandidateSocketChanged | PromotionError::CandidateNotReady)
        ),
        "replaced candidate socket must never satisfy the Ready gate: {promotion:?}"
    );
    assert_eq!(
        coordinator.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active
    );
    assert!(hold.is_held());

    drop(replacement);
    drop(coordinator);
    active.stop();
}

#[test]
fn promotion_switches_only_the_requested_ecu_and_acks_a_held_candidate_cycle() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let (mut coordinator, mut active, active_directory) =
        start_external_coordinator(&root, &candidate_directory);
    let candidate_coordinator = candidate_directory.join("coordinator.sock");
    let mut candidate = ExternalWorkerProcesses::start_one_with_allowed(
        "engine",
        &candidate_directory,
        &candidate_coordinator,
        Some(&active_directory.join("coordinator.sock")),
    );
    wait_for_path(&candidate_directory.join("engine.sock"));
    run_candidate_until_ready(&mut coordinator);

    let hold = StickyOutputHold::new(false);
    assert!(!hold.request_and_wait(Duration::ZERO));
    let mut platform = HeldRecordingPlatform {
        inner: RecordingPlatform::default(),
        hold: hold.clone(),
    };
    hold.apply(&mut platform.inner, VehicleActuation::default())
        .unwrap();
    let initial_zero_generation = hold.zero_apply_generation();
    let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
    let (completion, response) = std::sync::mpsc::sync_channel(1);
    request_tx
        .send(CandidateRouteRequest {
            ecu: VirtualEcuId::Engine,
            operation: CandidateRouteOperation::Promote,
            deadline: Instant::now() + Duration::from_secs(5),
            completion,
        })
        .unwrap();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator)
            .with_route_control(hold.clone(), request_rx);

    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        executor.host_cycle(Duration::from_millis(100)).unwrap();
        if let Ok(result) = response.try_recv() {
            result.unwrap_or_else(|error| {
                panic!(
                    "{error}; candidate socket={:?}, worker status={:?}",
                    fs::symlink_metadata(candidate_directory.join("engine.sock")),
                    candidate.0[0].try_wait()
                )
            });
            break;
        }
        assert!(Instant::now() < deadline, "promotion was not acknowledged");
        thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Candidate
    );
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Brake),
        EcuRoute::Active
    );
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Steering),
        EcuRoute::Active
    );
    assert!(hold.is_held());
    assert!(hold.zero_apply_generation() > initial_zero_generation);
    assert_eq!(
        executor.platform().inner.applied.last().unwrap().command,
        Default::default()
    );

    for _ in 0..3 {
        executor.host_cycle(Duration::from_millis(100)).unwrap();
    }

    let generation_before_reversion = hold.zero_apply_generation();
    let (completion, response) = std::sync::mpsc::sync_channel(1);
    request_tx
        .send(CandidateRouteRequest {
            ecu: VirtualEcuId::Engine,
            operation: CandidateRouteOperation::ReturnToActive,
            deadline: Instant::now() + Duration::from_secs(5),
            completion,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        executor.host_cycle(Duration::from_millis(100)).unwrap();
        if let Ok(result) = response.try_recv() {
            result.unwrap_or_else(|error| panic!("route reversion was rejected: {error}"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "route reversion was not acknowledged"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Active
    );
    assert!(hold.is_held());
    assert!(hold.zero_apply_generation() > generation_before_reversion);
    assert_eq!(
        executor.platform().inner.applied.last().unwrap().command,
        Default::default()
    );

    // The active ECU remains usable after the trial candidate exits.
    candidate.stop();
    for _ in 0..3 {
        executor.host_cycle(Duration::from_millis(100)).unwrap();
    }

    drop(executor);
    active.stop();
}

#[test]
fn candidate_active_cycle_failure_stops_vehicle_and_keeps_output_hold_latched() {
    let root = TempDirectory::new();
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let responder = CandidateResponder::start(&candidate_directory.join("engine.sock"), true);
    let (mut coordinator, mut active, _) = start_external_coordinator(&root, &candidate_directory);
    run_candidate_until_ready(&mut coordinator);
    let hold = StickyOutputHold::new(false);
    assert!(!hold.request_and_wait(Duration::ZERO));
    let mut platform = HeldRecordingPlatform {
        inner: RecordingPlatform::default(),
        hold: hold.clone(),
    };
    hold.apply(&mut platform.inner, VehicleActuation::default())
        .unwrap();
    let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
    let (completion, response) = std::sync::mpsc::sync_channel(1);
    request_tx
        .send(CandidateRouteRequest {
            ecu: VirtualEcuId::Engine,
            operation: CandidateRouteOperation::Promote,
            deadline: Instant::now() + Duration::from_secs(5),
            completion,
        })
        .unwrap();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator)
            .with_route_control(hold.clone(), request_rx);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut failed = false;
    loop {
        match executor.host_cycle(Duration::from_millis(100)) {
            Ok(()) => {}
            Err(_) => {
                failed = true;
                break;
            }
        }
        if response.try_recv().is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "candidate cycle did not fail closed"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        failed,
        "candidate protocol failure must fail the active cycle"
    );
    assert_eq!(
        executor.route_for_ecu(VirtualEcuId::Engine),
        EcuRoute::Candidate
    );
    assert!(hold.is_held());
    assert!(executor.platform().inner.stops > 0);
    assert!(executor
        .platform()
        .inner
        .applied
        .iter()
        .all(|actuation| actuation.command == Default::default()));

    drop(executor);
    drop(responder);
    active.stop();
}

#[test]
fn malformed_candidate_response_flood_is_isolated_from_active_workers() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let candidate_socket = UnixDatagram::bind(&candidate_path).unwrap();
    fs::set_permissions(&candidate_path, fs::Permissions::from_mode(0o600)).unwrap();
    candidate_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (flooded_tx, flooded_rx) = std::sync::mpsc::channel();
    let malformed_candidate = thread::spawn(move || {
        let mut request = [0; 1024];
        let (_, coordinator_addr) = candidate_socket.recv_from(&mut request).unwrap();
        let coordinator_path = coordinator_addr.as_pathname().unwrap();
        candidate_socket.set_nonblocking(true).unwrap();
        for _ in 0..128 {
            if candidate_socket
                .send_to(b"invalid-candidate-response", coordinator_path)
                .is_err()
            {
                break;
            }
        }
        let _ = flooded_tx.send(());
    });
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    wait_for_path(&candidate_directory.join("coordinator.sock"));
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    let engine_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let brake_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let steering_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let inputs = [
        engine_input.as_slice(),
        brake_input.as_slice(),
        steering_input.as_slice(),
    ];
    for cycle_index in 0..2 {
        let cycle_id = coordinator.next_cycle_id().unwrap();
        let cycle_started = Instant::now();
        let result =
            coordinator.exchange_cycle(cycle_id, inputs, cycle_started + Duration::from_millis(10));
        assert!(
            result.is_ok(),
            "candidate protocol error stopped active workers: {result:?}"
        );
        assert!(
            cycle_started.elapsed() < Duration::from_millis(10),
            "candidate maintenance consumed the 10 ms active cycle budget"
        );
        if cycle_index == 0 {
            // The first cycle queues the shadow request. Wait until the candidate flood is on
            // its private coordinator socket before measuring the active cycle that drains it.
            flooded_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        }
    }
    assert_eq!(
        wait_for_candidate_status(&coordinator, |status| {
            matches!(status, CandidateStatus::Failed(_))
        }),
        CandidateStatus::Failed(CandidateFailure::Protocol)
    );
    malformed_candidate.join().unwrap();
    drop(coordinator);
    active.stop();
}

#[test]
fn duplicate_candidate_response_does_not_consume_active_replies() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let candidate_directory = root.path().join("candidates");
    fs::create_dir(&candidate_directory).unwrap();
    fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let candidate_path = candidate_directory.join("engine.sock");
    let candidate_socket = UnixDatagram::bind(&candidate_path).unwrap();
    fs::set_permissions(&candidate_path, fs::Permissions::from_mode(0o600)).unwrap();
    candidate_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let duplicate_candidate = thread::spawn(move || {
        let mut request = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
        let (received, coordinator_addr) = candidate_socket.recv_from(&mut request).unwrap();
        let coordinator_path = coordinator_addr.as_pathname().unwrap();
        let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
        let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];
        let response_len = worker
            .handle_datagram(&request[..received], &mut response)
            .unwrap();
        candidate_socket
            .send_to(&response[..response_len], coordinator_path)
            .unwrap();
        candidate_socket
            .send_to(&response[..response_len], coordinator_path)
            .unwrap();
    });
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = candidate_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    wait_for_path(&candidate_directory.join("coordinator.sock"));
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    let engine_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let brake_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let steering_input = [sdv_core::bus::Frame::Safety(Default::default())];
    let inputs = [
        engine_input.as_slice(),
        brake_input.as_slice(),
        steering_input.as_slice(),
    ];

    for cycle_index in 0..2 {
        let cycle_id = coordinator.next_cycle_id().unwrap();
        let outputs = coordinator
            .exchange_cycle(
                cycle_id,
                inputs,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        assert!(outputs
            .iter()
            .all(|output| matches!(output.alive, sdv_core::bus::Frame::Alive { .. })));
        if cycle_index == 0 {
            // Give the background shadow coordinator time to consume the first bounded snapshot;
            // otherwise queue saturation can correctly reset stability before it observes the
            // duplicate pair under test.
            thread::sleep(Duration::from_millis(10));
        }
    }
    duplicate_candidate.join().unwrap();
    assert_eq!(
        wait_for_candidate_status(&coordinator, |status| {
            matches!(status, CandidateStatus::Failed(_))
        }),
        CandidateStatus::Failed(CandidateFailure::DuplicateResponse)
    );
    drop(coordinator);
    active.stop();
}

#[test]
fn external_coordinator_waits_for_workers_and_cleans_up_only_its_socket() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        thread::spawn(move || EcuProcessCoordinator::connect_external(socket_directory))
    };
    wait_for_path(&coordinator_path);
    let processes = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let coordinator = coordinator_thread.join().unwrap().unwrap();
    drop(coordinator);

    assert!(!coordinator_path.exists());
    for ecu in ["engine", "brake", "steering"] {
        assert!(socket_directory.join(format!("{ecu}.sock")).exists());
    }
    drop(processes);
}

#[test]
fn external_worker_failure_stops_vehicle_and_preserves_other_worker_sockets() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        thread::spawn(move || EcuProcessCoordinator::connect_external(socket_directory))
    };
    wait_for_path(&coordinator_path);
    let processes = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let coordinator = coordinator_thread.join().unwrap().unwrap();
    fs::remove_file(socket_directory.join("engine.sock")).unwrap();
    let platform = RecordingPlatform::default();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

    let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || false);
    assert!(matches!(
        result,
        Err(RunError::Cycle {
            source: VehicleCycleError::Workers {
                source: ExternalEcuCycleError::Dispatch(CoordinatorError::InvalidWorkerSocket(_)),
                ..
            },
            ..
        })
    ));
    assert!(executor.platform().applied.is_empty());
    assert!(executor.platform().stops >= 1);
    drop(executor);
    assert!(!coordinator_path.exists());
    assert!(socket_directory.join("brake.sock").exists());
    assert!(socket_directory.join("steering.sock").exists());
    drop(processes);
}

#[test]
fn external_connect_timeout_cleans_only_the_coordinator_socket() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let result = EcuProcessCoordinator::connect_external(&socket_directory);
    assert!(matches!(
        result,
        Err(CoordinatorError::WorkerStartupTimeout(
            sdv_core::runtime::VirtualEcuId::Engine
        ))
    ));
    assert!(!socket_directory.join("coordinator.sock").exists());
    assert!(!socket_directory.join("engine.sock").exists());
}

#[test]
fn external_connect_requires_an_existing_private_socket_directory() {
    let root = TempDirectory::new();
    let socket_directory = root.path().join("workers");
    assert!(matches!(
        EcuProcessCoordinator::connect_external(&socket_directory),
        Err(CoordinatorError::Io(_)) | Err(CoordinatorError::UnsafeSocketPath)
    ));
    assert!(!socket_directory.exists());
}

#[test]
fn invalid_candidate_socket_configuration_does_not_block_active_cycles() {
    let root = TempDirectory::new();
    let socket_directory = private_worker_directory(&root);
    let coordinator_path = socket_directory.join("coordinator.sock");
    let coordinator_thread = {
        let socket_directory = socket_directory.clone();
        let candidate_directory = socket_directory.clone();
        thread::spawn(move || {
            EcuProcessCoordinator::connect_external_with_candidates(
                socket_directory,
                candidate_directory,
            )
        })
    };
    wait_for_path(&coordinator_path);
    let mut active = ExternalWorkerProcesses::start(&socket_directory, &coordinator_path);
    let mut coordinator = coordinator_thread.join().unwrap().unwrap();
    assert_eq!(
        coordinator.candidate_statuses()[0],
        CandidateStatus::Failed(CandidateFailure::UnsafeSocket)
    );

    let input = [sdv_core::bus::Frame::Safety(Default::default())];
    let cycle_id = coordinator.next_cycle_id().unwrap();
    let outputs = coordinator
        .exchange_cycle(
            cycle_id,
            [input.as_slice(), input.as_slice(), input.as_slice()],
            Instant::now() + Duration::from_millis(100),
        )
        .unwrap();
    assert!(outputs
        .iter()
        .all(|output| matches!(output.alive, sdv_core::bus::Frame::Alive { .. })));
    drop(coordinator);
    active.stop();
}

#[test]
fn a_missing_worker_path_stops_without_applying_partial_or_stale_outputs() {
    let root = TempDirectory::new();
    let coordinator = spawn_workers(&root);
    fs::remove_file(root.path().join("workers/engine.sock")).unwrap();
    let platform = RecordingPlatform::default();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

    let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || false);
    assert!(matches!(
        result,
        Err(RunError::Cycle {
            source: VehicleCycleError::Workers {
                source: ExternalEcuCycleError::Dispatch(CoordinatorError::InvalidWorkerSocket(_)),
                ..
            },
            ..
        })
    ));
    assert!(executor.platform().applied.is_empty());
    assert!(executor.platform().stops >= 1);
}

fn fake_worker_binary(root: &TempDirectory, mode: &str) -> PathBuf {
    let path = root.path().join(format!("fake-worker-{mode}.py"));
    let source = format!(
        r#"#!/usr/bin/env python3
import os, socket, struct, sys, time, zlib
args = dict(zip(sys.argv[1::2], sys.argv[2::2]))
worker = args['--worker-socket']
ecu = args['--ecu']
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.bind(worker)
os.chmod(worker, 0o600)
s.connect(args['--coordinator-socket'])
if '{mode}' == 'startup-fail' and ecu == 'brake':
    os.chmod(worker, 0o644)

def wire_frame(kind, payload):
    frame = b'SDVW' + bytes((1, kind, len(payload))) + payload
    return frame + struct.pack('<I', zlib.crc32(frame) & 0xffffffff)

while True:
    request = s.recv(2048)
    if '{mode}' == 'malformed':
        s.send(b'bad')
    elif '{mode}' == 'silent' or ('{mode}' == 'partial' and ecu == 'steering'):
        time.sleep(1)
    else:
        response_ecu = 'brake' if '{mode}' == 'wrong-ecu' and ecu == 'engine' else ecu
        ecu_number = {{'engine': 1, 'brake': 2, 'steering': 3}}[response_ecu]
        command_kind = {{'engine': 5, 'brake': 6, 'steering': 7}}[response_ecu]
        alive_index = {{'engine': 0, 'brake': 1, 'steering': 2}}[response_ecu]
        cycle = request[6:14]
        cycle_number = int.from_bytes(cycle, 'little')
        if '{mode}' == 'stale-cycle':
            cycle = ((cycle_number - 1) % (1 << 64)).to_bytes(8, 'little')
        elif '{mode}' == 'future-cycle':
            cycle = ((cycle_number + 1) % (1 << 64)).to_bytes(8, 'little')
        command = wire_frame(command_kind, struct.pack('<f', 0.0))
        alive = wire_frame(3, bytes((alive_index,)) + cycle)
        response = bytearray(15)
        response[0:4] = b'SDVP'
        response[4] = 1
        response[5] = ecu_number
        response[6:14] = cycle
        response[14] = 2
        for frame in (command, alive):
            response.append(len(frame))
            response.extend(frame)
        response.extend(struct.pack('<I', zlib.crc32(response) & 0xffffffff))
        s.send(response)
        open(worker + '.replied', 'wb').close()
"#
    );
    fs::write(&path, source).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn malformed_worker_response_stops_without_applying_any_response() {
    let root = TempDirectory::new();
    let coordinator = EcuProcessCoordinator::spawn(
        fake_worker_binary(&root, "malformed"),
        root.path().join("workers"),
    )
    .unwrap();
    let platform = RecordingPlatform::default();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

    let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || false);
    assert!(matches!(
        result,
        Err(RunError::Cycle {
            source: VehicleCycleError::Workers {
                source: ExternalEcuCycleError::Dispatch(CoordinatorError::WorkerProtocol(_)),
                ..
            },
            ..
        })
    ));
    assert!(executor.platform().applied.is_empty());
    assert!(executor.platform().stops >= 1);
}

#[test]
fn wrong_ecu_and_stale_or_future_cycle_responses_fail_closed() {
    for (mode, wrong_error) in [
        ("wrong-ecu", "ecu"),
        ("stale-cycle", "cycle"),
        ("future-cycle", "cycle"),
    ] {
        let root = TempDirectory::new();
        let coordinator = EcuProcessCoordinator::spawn(
            fake_worker_binary(&root, mode),
            root.path().join("workers"),
        )
        .unwrap();
        let platform = RecordingPlatform::default();
        let mut executor =
            EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

        let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || false);
        match (wrong_error, result) {
            (
                "ecu",
                Err(RunError::Cycle {
                    source:
                        VehicleCycleError::Workers {
                            source:
                                ExternalEcuCycleError::Dispatch(CoordinatorError::WrongWorkerEcu),
                            ..
                        },
                    ..
                }),
            ) => {}
            (
                "cycle",
                Err(RunError::Cycle {
                    source:
                        VehicleCycleError::Workers {
                            source: ExternalEcuCycleError::Dispatch(CoordinatorError::WrongCycleId),
                            ..
                        },
                    ..
                }),
            ) => {}
            (_, other) => panic!("unexpected result for {mode}: {other:?}"),
        }
        assert!(executor.platform().applied.is_empty());
        assert!(executor.platform().stops >= 1);
    }
}

#[test]
fn silent_worker_times_out_with_the_host_remaining_budget_and_stops() {
    let root = TempDirectory::new();
    let coordinator = EcuProcessCoordinator::spawn(
        fake_worker_binary(&root, "silent"),
        root.path().join("workers"),
    )
    .unwrap();
    let platform = RecordingPlatform::default();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

    let result = runtime_host::run(&mut executor, &mut SystemClock::default(), || false);
    assert!(matches!(
        result,
        Err(RunError::Cycle {
            source: VehicleCycleError::Workers {
                source: ExternalEcuCycleError::Dispatch(CoordinatorError::DeadlineExceeded),
                ..
            },
            ..
        })
    ));
    assert!(executor.platform().applied.is_empty());
    assert!(executor.platform().stops >= 1);
}

#[test]
fn partial_worker_responses_timeout_without_applying_any_command() {
    let root = TempDirectory::new();
    let coordinator = EcuProcessCoordinator::spawn(
        fake_worker_binary(&root, "partial"),
        root.path().join("workers"),
    )
    .unwrap();
    let platform = RecordingPlatform::default();
    let mut executor =
        EcuProcessVehicleExecutor::new(VehicleRuntime::default(), platform, coordinator);

    let result = executor.host_cycle(Duration::from_millis(100));
    assert!(matches!(
        result,
        Err(VehicleCycleError::Workers {
            source: ExternalEcuCycleError::Dispatch(CoordinatorError::DeadlineExceeded),
            ..
        })
    ));
    assert!(root.path().join("workers/engine.sock.replied").exists());
    assert!(root.path().join("workers/brake.sock.replied").exists());
    assert!(!root.path().join("workers/steering.sock.replied").exists());
    assert!(executor.platform().applied.is_empty());
    assert!(executor.platform().stops >= 1);
}

#[test]
fn socket_cleanup_keeps_replacement_paths_with_different_inodes() {
    let root = TempDirectory::new();
    let coordinator = spawn_workers(&root);
    let worker_path = root.path().join("workers/engine.sock");
    let coordinator_path = root.path().join("workers/coordinator.sock");
    fs::remove_file(&worker_path).unwrap();
    fs::remove_file(&coordinator_path).unwrap();
    let replacement_worker = UnixDatagram::bind(&worker_path).unwrap();
    let replacement_coordinator = UnixDatagram::bind(&coordinator_path).unwrap();
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&coordinator_path, fs::Permissions::from_mode(0o600)).unwrap();

    drop(coordinator);
    assert!(fs::symlink_metadata(&worker_path).is_ok());
    assert!(fs::symlink_metadata(&coordinator_path).is_ok());
    drop(replacement_worker);
    drop(replacement_coordinator);
}
