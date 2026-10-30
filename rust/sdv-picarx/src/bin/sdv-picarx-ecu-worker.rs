use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sdv_core::config::Config;
use sdv_core::runtime::VirtualEcuId;
use sdv_picarx::virtual_ecu_worker::{VirtualEcuWorker, WorkerError};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

const SOCKET_MODE: u32 = 0o600;
const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(100);
const UNSAFE_WRITE_BITS: u32 = 0o022;

struct BoundWorkerSocket {
    socket: UnixDatagram,
    path: PathBuf,
    device: u64,
    inode: u64,
    owner: u32,
}

struct WorkerSocketPathGuard {
    path: PathBuf,
    owner: u32,
    identity: Option<(u64, u64)>,
}

impl WorkerSocketPathGuard {
    fn new(path: PathBuf, owner: u32) -> Self {
        Self {
            path,
            owner,
            identity: None,
        }
    }

    fn record_identity(&mut self, metadata: &fs::Metadata) {
        self.identity = Some((metadata.dev(), metadata.ino()));
    }

    fn disarm(&mut self) {
        self.identity = None;
        self.path.clear();
    }
}

impl Drop for WorkerSocketPathGuard {
    fn drop(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            let is_our_socket = metadata.file_type().is_socket()
                && metadata.uid() == self.owner
                && self
                    .identity
                    .map(|(device, inode)| metadata.dev() == device && metadata.ino() == inode)
                    .unwrap_or(true);
            if is_our_socket {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

impl BoundWorkerSocket {
    fn bind(worker_path: PathBuf, coordinator_path: &Path) -> io::Result<Self> {
        if worker_path == coordinator_path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker and coordinator socket paths must differ",
            ));
        }
        let owner = unsafe { libc::geteuid() };
        validate_socket_parent(&worker_path, owner)?;
        validate_coordinator_socket(coordinator_path, owner)?;
        match fs::symlink_metadata(&worker_path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "worker socket path already exists; refusing to replace it",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let socket = UnixDatagram::bind(&worker_path)?;
        let mut path_guard = WorkerSocketPathGuard::new(worker_path.clone(), owner);
        let metadata = fs::symlink_metadata(&worker_path)?;
        if !metadata.file_type().is_socket() || metadata.uid() != owner {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "bound worker path is not an owned Unix socket",
            ));
        }
        path_guard.record_identity(&metadata);
        let bound = Self {
            socket,
            path: worker_path,
            device: metadata.dev(),
            inode: metadata.ino(),
            owner,
        };
        path_guard.disarm();

        fs::set_permissions(&bound.path, fs::Permissions::from_mode(SOCKET_MODE))?;
        validate_bound_socket(&bound)?;
        bound.socket.connect(coordinator_path)?;
        bound.socket.set_read_timeout(Some(SOCKET_READ_TIMEOUT))?;
        Ok(bound)
    }
}

impl Drop for BoundWorkerSocket {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_socket()
                && metadata.uid() == self.owner
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn main() {
    if let Err(error) = run(env::args_os().skip(1)) {
        eprintln!("sdv-picarx-ecu-worker: {error}");
        std::process::exit(1);
    }
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let mut options = parse_named_options(args)?;
    let ecu_id = parse_ecu_id(&required_option(&mut options, "--ecu")?)?;
    let worker_path = PathBuf::from(required_option(&mut options, "--worker-socket")?);
    let coordinator_path = PathBuf::from(required_option(&mut options, "--coordinator-socket")?);

    let shutdown = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, Arc::clone(&shutdown))
        .map_err(|error| format!("cannot register SIGINT handler: {error}"))?;
    flag::register(SIGTERM, Arc::clone(&shutdown))
        .map_err(|error| format!("cannot register SIGTERM handler: {error}"))?;

    let socket = BoundWorkerSocket::bind(worker_path, &coordinator_path)
        .map_err(|error| format!("cannot bind worker socket: {error}"))?;
    let mut worker = VirtualEcuWorker::new(ecu_id, &Config::default());
    while !shutdown.load(Ordering::Relaxed) {
        match worker.serve_once(&socket.socket) {
            Ok(()) => {}
            Err(WorkerError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(WorkerError::Io(error)) => {
                return Err(format!("worker socket failed: {error}"));
            }
            Err(error) => {
                eprintln!("sdv-picarx-ecu-worker: rejected datagram: {error}");
            }
        }
    }
    Ok(())
}

fn parse_named_options(
    args: impl IntoIterator<Item = OsString>,
) -> Result<BTreeMap<String, OsString>, String> {
    let accepted = ["--ecu", "--worker-socket", "--coordinator-socket"];
    let mut options = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        let name = option
            .to_str()
            .ok_or_else(|| format!("option must be valid UTF-8\n{}", usage()))?;
        if !accepted.contains(&name) {
            return Err(format!("unknown option: {name}\n{}", usage()));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {name}\n{}", usage()))?;
        if options.insert(name.to_owned(), value).is_some() {
            return Err(format!(
                "option supplied more than once: {name}\n{}",
                usage()
            ));
        }
    }
    Ok(options)
}

fn required_option(
    options: &mut BTreeMap<String, OsString>,
    name: &str,
) -> Result<OsString, String> {
    options
        .remove(name)
        .ok_or_else(|| format!("missing required option {name}\n{}", usage()))
}

fn parse_ecu_id(value: &OsStr) -> Result<VirtualEcuId, String> {
    match value.to_str() {
        Some("engine") => Ok(VirtualEcuId::Engine),
        Some("brake") => Ok(VirtualEcuId::Brake),
        Some("steering") => Ok(VirtualEcuId::Steering),
        _ => Err(format!(
            "--ecu must be one of engine, brake, or steering\n{}",
            usage()
        )),
    }
}

fn usage() -> &'static str {
    "usage: sdv-picarx-ecu-worker --ecu <engine|brake|steering> --worker-socket <path> --coordinator-socket <path>"
}

fn validate_socket_parent(path: &Path, owner: u32) -> io::Result<()> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket paths must be absolute and cannot contain parent traversal",
        ));
    }

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"))?;
    let mut current = parent;
    let mut immediate = true;
    loop {
        let metadata = fs::symlink_metadata(current)?;
        let mode = metadata.permissions().mode();
        let owned_by_trusted_account = metadata.uid() == owner || metadata.uid() == 0;
        let protected_sticky_directory = metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.file_type().is_dir()
            || !owned_by_trusted_account
            || (immediate && metadata.uid() != owner)
            || (mode & UNSAFE_WRITE_BITS != 0 && !protected_sticky_directory)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("unsafe socket parent directory: {}", current.display()),
            ));
        }
        if current == Path::new("/") {
            break;
        }
        current = current.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid socket parent chain",
            )
        })?;
        immediate = false;
    }
    Ok(())
}

fn validate_coordinator_socket(path: &Path, owner: u32) -> io::Result<()> {
    validate_socket_parent(path, owner)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "coordinator path must be an owner-only Unix socket owned by this user",
        ));
    }
    Ok(())
}

fn validate_bound_socket(bound: &BoundWorkerSocket) -> io::Result<()> {
    let metadata = fs::symlink_metadata(&bound.path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != bound.owner
        || metadata.permissions().mode() & 0o777 != SOCKET_MODE
        || metadata.dev() != bound.device
        || metadata.ino() != bound.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "worker socket identity or permissions changed during startup",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    fn test_socket_path(name: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "sdv-picarx-worker-path-guard-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        (directory.clone(), directory.join("worker.sock"))
    }

    #[test]
    fn ecu_id_accepts_only_the_three_worker_tasks() {
        assert_eq!(
            parse_ecu_id(OsStr::new("engine")).unwrap(),
            VirtualEcuId::Engine
        );
        assert_eq!(
            parse_ecu_id(OsStr::new("brake")).unwrap(),
            VirtualEcuId::Brake
        );
        assert_eq!(
            parse_ecu_id(OsStr::new("steering")).unwrap(),
            VirtualEcuId::Steering
        );
        assert!(parse_ecu_id(OsStr::new("vehicle")).is_err());
    }

    #[test]
    fn named_options_reject_unknown_duplicate_and_missing_values() {
        assert!(parse_named_options(["--other".into(), "x".into()]).is_err());
        assert!(parse_named_options(["--ecu".into()]).is_err());
        assert!(parse_named_options([
            "--ecu".into(),
            "engine".into(),
            "--ecu".into(),
            "brake".into()
        ])
        .is_err());
    }

    #[test]
    fn path_guard_cleans_a_socket_when_startup_fails_before_metadata_is_recorded() {
        let (directory, path) = test_socket_path("metadata-failure");
        let _socket = UnixDatagram::bind(&path).unwrap();
        let owner = unsafe { libc::geteuid() };

        drop(WorkerSocketPathGuard::new(path.clone(), owner));

        assert!(fs::symlink_metadata(&path).is_err());
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn path_guard_does_not_remove_a_replacement_socket() {
        let (directory, path) = test_socket_path("replacement");
        let original_socket = UnixDatagram::bind(&path).unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let mut guard = WorkerSocketPathGuard::new(path.clone(), unsafe { libc::geteuid() });
        guard.record_identity(&metadata);

        fs::remove_file(&path).unwrap();
        let replacement_socket = UnixDatagram::bind(&path).unwrap();
        drop(guard);

        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
        drop(replacement_socket);
        fs::remove_file(path).unwrap();
        drop(original_socket);
        fs::remove_dir(directory).unwrap();
    }
}
