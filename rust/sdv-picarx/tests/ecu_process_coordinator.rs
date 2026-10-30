use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sdv_core::model::VehicleState;
use sdv_core::platform::HostCycleExecutor;
use sdv_core::platform::{PlatformInputs, VehiclePlatform};
use sdv_core::rte::DriverInput;
use sdv_core::runtime::{ExternalEcuCycleError, VehicleActuation, VehicleRuntime};
use sdv_core::runtime_host::{self, RunError, SystemClock};
use sdv_picarx::ecu_process_coordinator::{
    EcuProcessCoordinator, EcuProcessVehicleExecutor, Error as CoordinatorError, VehicleCycleError,
};

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
