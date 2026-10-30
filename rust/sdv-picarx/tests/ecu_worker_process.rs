use sdv_core::bus::{Frame, ALIVE_ENGINE_INDEX};
use sdv_core::rte::{DriverInput, Safety};
use sdv_core::runtime::VirtualEcuId;
use sdv_picarx::virtual_ecu_worker::{decode_response, encode_request, WORKER_DATAGRAM_MAX_SIZE};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const WORKER_BINARY: &str = env!("CARGO_BIN_EXE_sdv-picarx-ecu-worker");

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sdv-picarx-worker-process-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn socket(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bind_coordinator(path: &PathBuf) -> UnixDatagram {
    let socket = UnixDatagram::bind(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    socket
}

fn spawn_worker(worker_path: &PathBuf, coordinator_path: &PathBuf) -> Child {
    Command::new(WORKER_BINARY)
        .args([
            "--ecu",
            "engine",
            "--worker-socket",
            worker_path.to_str().unwrap(),
            "--coordinator-socket",
            coordinator_path.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait_for_worker_socket(child: &mut Child, path: &PathBuf) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            if let Some(mut stream) = child.stderr.take() {
                use std::io::Read;
                stream.read_to_string(&mut stderr).unwrap();
            }
            panic!("worker exited during startup ({status}): {stderr}");
        }
        if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("worker socket did not appear: {}", path.display());
}

fn request_engine_cycle(coordinator: &UnixDatagram, worker_path: &PathBuf, cycle_id: u64) {
    let frames = [
        Frame::Safety(Safety::default()),
        Frame::DriverInput(DriverInput {
            throttle: 0.5,
            brake: 0.0,
            steer: 0.0,
        }),
    ];
    let mut request = [0; WORKER_DATAGRAM_MAX_SIZE];
    let request_len =
        encode_request(VirtualEcuId::Engine, cycle_id, &frames, &mut request).unwrap();
    coordinator
        .send_to(&request[..request_len], worker_path)
        .unwrap();

    let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];
    let (response_len, address) = coordinator.recv_from(&mut response).unwrap();
    assert_eq!(address.as_pathname(), Some(worker_path.as_path()));
    let decoded = decode_response(&response[..response_len]).unwrap();
    assert_eq!(decoded.ecu_id, VirtualEcuId::Engine);
    assert_eq!(decoded.cycle_id, cycle_id);
    assert!(matches!(decoded.command, Frame::CmdDrive(_)));
    assert!(matches!(
        decoded.alive,
        Frame::Alive {
            index: ALIVE_ENGINE_INDEX,
            counter
        } if counter == cycle_id
    ));
}

fn stop_with_signal(child: &mut Child, signal: i32, worker_path: &PathBuf) {
    let pid = libc::pid_t::try_from(child.id()).unwrap();
    assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("worker did not shut down after signal {signal}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "worker exited with {status}");
    assert!(
        fs::symlink_metadata(worker_path).is_err(),
        "worker socket path was not removed after shutdown"
    );
}

#[test]
fn process_serves_frames_restricts_socket_mode_and_cleans_up_on_sigint_and_sigterm() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let directory = TestDirectory::new();
        let coordinator_path = directory.socket("coordinator.sock");
        let worker_path = directory.socket("engine.sock");
        let coordinator = bind_coordinator(&coordinator_path);
        let mut child = spawn_worker(&worker_path, &coordinator_path);

        wait_for_worker_socket(&mut child, &worker_path);
        let metadata = fs::symlink_metadata(&worker_path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

        request_engine_cycle(&coordinator, &worker_path, 1);
        assert!(child.try_wait().unwrap().is_none());
        stop_with_signal(&mut child, signal, &worker_path);
        assert!(coordinator_path.exists());
    }
}

fn run_worker_output(worker_path: &PathBuf, coordinator_path: &PathBuf) -> Output {
    Command::new(WORKER_BINARY)
        .args([
            "--ecu",
            "engine",
            "--worker-socket",
            worker_path.to_str().unwrap(),
            "--coordinator-socket",
            coordinator_path.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

#[test]
fn process_refuses_existing_worker_socket_paths_without_replacing_them() {
    let directory = TestDirectory::new();
    let coordinator_path = directory.socket("coordinator.sock");
    let _coordinator = bind_coordinator(&coordinator_path);

    let regular_file = directory.socket("regular.sock");
    fs::write(&regular_file, b"keep this file").unwrap();
    let output = run_worker_output(&regular_file, &coordinator_path);
    assert!(!output.status.success());
    assert_eq!(fs::read(&regular_file).unwrap(), b"keep this file");

    let target = directory.socket("target");
    fs::write(&target, b"symlink target remains").unwrap();
    let symlink = directory.socket("symlink.sock");
    std::os::unix::fs::symlink(&target, &symlink).unwrap();
    let output = run_worker_output(&symlink, &coordinator_path);
    assert!(!output.status.success());
    assert!(fs::symlink_metadata(&symlink)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"symlink target remains");

    let existing_socket_path = directory.socket("existing.sock");
    let existing_socket = UnixDatagram::bind(&existing_socket_path).unwrap();
    fs::set_permissions(&existing_socket_path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::symlink_metadata(&existing_socket_path).unwrap();
    let output = run_worker_output(&existing_socket_path, &coordinator_path);
    assert!(!output.status.success());
    let after = fs::symlink_metadata(&existing_socket_path).unwrap();
    assert!(after.file_type().is_socket());
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    drop(existing_socket);
}

#[test]
fn process_rejects_a_symlinked_coordinator_without_leaving_a_worker_socket() {
    let directory = TestDirectory::new();
    let coordinator_target = directory.socket("coordinator-target");
    fs::write(&coordinator_target, b"not a socket").unwrap();
    let coordinator_link = directory.socket("coordinator.sock");
    std::os::unix::fs::symlink(&coordinator_target, &coordinator_link).unwrap();
    let worker_path = directory.socket("engine.sock");

    let output = run_worker_output(&worker_path, &coordinator_link);
    assert!(!output.status.success());
    assert!(fs::symlink_metadata(&worker_path).is_err());
    assert!(fs::symlink_metadata(&coordinator_link)
        .unwrap()
        .file_type()
        .is_symlink());
}
