//! Raspberry Pi 5 Linux userspace adapters for A/B SDV application images.
//!
//! These adapters write only under the configured application-data directory. They do not
//! touch the running root filesystem, EEPROM, boot partition, or raw block devices. A systemd
//! launcher should call `select_for_boot`, start the returned slot's image, then call
//! `commit_trial` only after the SDV service passes its health checks.

use super::{
    BootControl, BootDecision, BootState, Ecu, ImageStorage, LastTrialResult, Slot, TrialImage,
    TrialOutcome, VersionString,
};
use crate::package::decode_sha256;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const RECORD_MAGIC: &[u8; 4] = b"SDVB";
const RECORD_HEADER_BYTES: usize = 16;
const MAX_BOOT_RECORD_BYTES: usize = 4096;
const IMAGE_FILE: &str = "sdv-image.bin";
const IMAGE_DIGEST_FILE: &str = "sdv-image.sha256";
const IMAGE_DIGEST_HEX_BYTES: usize = 64;
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const LOCK_UN: i32 = 8;
const TRIAL_HEALTH_STABILITY_WINDOW: Duration = Duration::from_secs(2);
const TRIAL_HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TRIAL_PROCESS_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_HOLD_REQUEST: &[u8] = b"SDVHOLD1\n";
const OUTPUT_HOLD_ACK: &[u8] = b"SDVHELD1\n";
const OUTPUT_HOLD_NOT_READY: &[u8] = b"SDVNOTREADY\n";
const OUTPUT_PROMOTION_REQUEST_PREFIX: &[u8] = b"SDVPROMOTE1 ";
const OUTPUT_REVERSION_REQUEST_PREFIX: &[u8] = b"SDVREVERT1 ";
const OUTPUT_HOLD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_HOLD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(7);
const HOLD_SOCKET_ENV: &str = "SDV_FOTA_HOLD_SOCKET";
const HOLD_LISTENER_FD_ENV: &str = "SDV_FOTA_HOLD_LISTENER_FD";
const HOLD_FD_MINIMUM: RawFd = 32;
// Linux O_NOFOLLOW: reject a final path component that is a symbolic link.
const O_NOFOLLOW: i32 = 0x20_000;
const UNSAFE_WRITE_BITS: u32 = 0o022;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn geteuid() -> u32;
    fn setgroups(size: usize, list: *const u32) -> i32;
    fn setgid(gid: u32) -> i32;
    fn setuid(uid: u32) -> i32;
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NoValidBootControl,
    AlreadyInitialized,
    TrialAlreadyPending,
    NoTrialPending,
    ImageWriteInProgress,
    InvalidImageWrite,
    InvalidRecord,
    InvalidBootImage,
    TrialImageExited,
    TrialHealthTimeout,
    TrailingPackageData,
    UpdateInProgress,
    InvalidHttpsUrl,
    InvalidClientCredentials,
    UnsafeClientCredentials,
    DownloadFailed,
    InvalidRunIdentity,
    InvalidTargetArguments,
    InvalidTargetLaunchMode,
    UnsafeStatePath,
    SlotIsNotActive,
    OutputHoldFailed,
    OutputHoldNotReady,
    CandidateRouteChangeFailed,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// File-backed package ingress for bounded streaming installation.
pub struct FilePackageSource {
    file: File,
}

impl FilePackageSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            file: File::open(path)?,
        })
    }
}

impl super::PackageSource for FilePackageSource {
    type Error = Error;

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Self::Error> {
        Read::read_exact(&mut self.file, buffer).map_err(Error::Io)
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        let mut trailing = [0u8; 1];
        if self.file.read(&mut trailing)? == 0 {
            Ok(())
        } else {
            Err(Error::TrailingPackageData)
        }
    }
}

/// HTTPS package ingress. `curl` performs system-CA TLS verification; redirects remain HTTPS
/// and the response body streams through stdout without a package-sized temporary file.
pub struct HttpsPackageSource {
    child: Option<Child>,
    stdout: ChildStdout,
}

impl HttpsPackageSource {
    pub fn open(url: &str) -> Result<Self, Error> {
        Self::open_with_client_identity(url, None, None)
    }

    pub fn open_with_client_identity(
        url: &str,
        client_certificate: Option<&Path>,
        client_key: Option<&Path>,
    ) -> Result<Self, Error> {
        let Some(host_and_path) = url.strip_prefix("https://") else {
            return Err(Error::InvalidHttpsUrl);
        };
        let authority = host_and_path.split('/').next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err(Error::InvalidHttpsUrl);
        }
        match (client_certificate, client_key) {
            (None, None) => {}
            (Some(certificate), Some(key)) => {
                validate_client_identity_file(certificate, false)?;
                validate_client_identity_file(key, true)?;
            }
            _ => return Err(Error::InvalidClientCredentials),
        }

        let mut command = Command::new("curl");
        command.args([
            "--disable",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "20",
            "--max-time",
            "900",
            "--output",
            "-",
        ]);
        if client_certificate.is_some() {
            // The device identity is authorized for the configured artifact origin only. Keep
            // redirects enabled for the package response, but fail before following any redirect
            // so curl cannot present this certificate to a different HTTPS host.
            command.args(["--max-redirs", "0"]);
        }
        if let (Some(certificate), Some(key)) = (client_certificate, client_key) {
            command.arg("--cert").arg(certificate).arg("--key").arg(key);
        }
        let mut child = command
            .arg("--")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or(Error::DownloadFailed)?;
        Ok(Self {
            child: Some(child),
            stdout,
        })
    }
}

fn validate_client_identity_file(path: &Path, private_key: bool) -> Result<(), Error> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Error::UnsafeClientCredentials);
    }

    let owner = unsafe { geteuid() };
    let parent = absolute_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("/"));
    validate_client_identity_parent_chain(parent, owner)?;

    let metadata = fs::symlink_metadata(&absolute_path)?;
    let mode = metadata.permissions().mode();
    if !metadata.file_type().is_file()
        || metadata.uid() != owner
        || mode & UNSAFE_WRITE_BITS != 0
        || (private_key && mode & 0o077 != 0)
    {
        return Err(Error::UnsafeClientCredentials);
    }
    Ok(())
}

/// Check every directory entry leading to the credential file. A private immediate directory is
/// insufficient when an untrusted writable ancestor could rename or replace that directory after
/// validation. Root-owned sticky directories such as /tmp are safe ancestors because users
/// cannot remove or replace entries they do not own there.
fn validate_client_identity_parent_chain(parent: &Path, owner: u32) -> Result<(), Error> {
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
            return Err(Error::UnsafeClientCredentials);
        }

        if current == Path::new("/") {
            break;
        }
        current = current.parent().ok_or(Error::UnsafeClientCredentials)?;
        immediate = false;
    }
    Ok(())
}

impl super::PackageSource for HttpsPackageSource {
    type Error = Error;

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Self::Error> {
        self.stdout.read_exact(buffer).map_err(Error::Io)
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        let mut trailing = [0u8; 1];
        if self.stdout.read(&mut trailing)? != 0 {
            return Err(Error::TrailingPackageData);
        }
        let status = self.child.take().ok_or(Error::DownloadFailed)?.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::DownloadFailed)
        }
    }
}

impl Drop for HttpsPackageSource {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Image payloads stored in versioned directories beneath the configured application-data root.
pub struct FileSlotStorage {
    root: PathBuf,
    pending: Option<PendingImage>,
    update_lock: Option<File>,
}

struct PendingImage {
    slot: Slot,
    directory: PathBuf,
    temporary: PathBuf,
    destination: PathBuf,
    file: File,
    expected_size: u32,
    written: u32,
    digest: sdv_sec::Sha256Context,
}

/// Serializes read-modify-write boot metadata transitions across installer and launcher processes.
struct BootRecordLock(File);

impl BootRecordLock {
    fn acquire(root: &Path) -> Result<Self, Error> {
        ensure_secure_directory(root, true)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(root.join(".fota-bootctl.lock"))?;
        validate_owned_file(&file).map_err(Error::Io)?;
        // SAFETY: this is a valid open file descriptor; flock is released when the guard drops.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX) } != 0 {
            return Err(Error::Io(io::Error::last_os_error()));
        }
        Ok(Self(file))
    }
}

impl Drop for BootRecordLock {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains open until after this Drop implementation returns.
        let _ = unsafe { flock(self.0.as_raw_fd(), LOCK_UN) };
    }
}

impl FileSlotStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            pending: None,
            update_lock: None,
        }
    }

    pub fn image_path(&self, slot: Slot) -> PathBuf {
        self.root
            .join("slots")
            .join(slot_name(slot))
            .join(IMAGE_FILE)
    }

    pub fn validate_executable_image(&self, slot: Slot) -> Result<(), Error> {
        ensure_secure_directory(&self.root, false)?;
        ensure_slot_directories(&self.root, slot, false)?;
        let metadata = fs::symlink_metadata(self.image_path(slot))?;
        if !metadata.file_type().is_file() {
            return Err(Error::UnsafeStatePath);
        }
        if metadata.len() == 0 || metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::InvalidBootImage);
        }
        if metadata.uid() != effective_uid()
            || metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
        {
            return Err(Error::UnsafeStatePath);
        }
        Ok(())
    }

    /// Record an integrity digest for an already provisioned executable image.
    pub fn record_image_integrity(&self, slot: Slot) -> Result<(), Error> {
        self.validate_executable_image(slot)?;
        let image = open_slot_image(&self.root, slot)?;
        let digest = hash_file(image)?;
        write_image_digest(&self.root, slot, &digest)
    }

    /// Validate the executable image and compare it with the digest recorded at provisioning or
    /// update time. This is required before the Linux launcher starts a slot.
    pub fn verify_executable_image(&self, slot: Slot) -> Result<(), Error> {
        self.validate_executable_image(slot)?;
        let expected = read_image_digest(&self.root, slot)?;
        let actual = hash_file(open_slot_image(&self.root, slot)?)?;
        if actual != expected {
            return Err(Error::InvalidBootImage);
        }
        Ok(())
    }
}

impl ImageStorage for FileSlotStorage {
    type Error = Error;

    fn begin_inactive_image(&mut self, active: Slot, image_size: u32) -> Result<Slot, Self::Error> {
        if self.pending.is_some() {
            return Err(Error::ImageWriteInProgress);
        }
        if image_size == 0 {
            return Err(Error::InvalidImageWrite);
        }
        ensure_secure_directory(&self.root, true)?;
        let new_lock = if self.update_lock.is_none() {
            Some(acquire_update_lock(&self.root)?)
        } else {
            None
        };
        let slot = active.other();
        let directory = self.root.join("slots").join(slot_name(slot));
        ensure_secure_directory(&self.root.join("slots"), true)?;
        ensure_secure_directory(&directory, true)?;
        let (temporary, file) = create_temporary_file(&directory, "sdv-image")?;
        let destination = directory.join(IMAGE_FILE);
        self.pending = Some(PendingImage {
            slot,
            directory,
            temporary,
            destination,
            file,
            expected_size: image_size,
            written: 0,
            digest: sdv_sec::Sha256Context::new(),
        });
        if let Some(lock) = new_lock {
            self.update_lock = Some(lock);
        }
        Ok(slot)
    }

    fn write_image_chunk(
        &mut self,
        slot: Slot,
        offset: u32,
        chunk: &[u8],
    ) -> Result<(), Self::Error> {
        let pending = self.pending.as_mut().ok_or(Error::InvalidImageWrite)?;
        let next = pending
            .written
            .checked_add(u32::try_from(chunk.len()).map_err(|_| Error::InvalidImageWrite)?)
            .ok_or(Error::InvalidImageWrite)?;
        if pending.slot != slot || offset != pending.written || next > pending.expected_size {
            return Err(Error::InvalidImageWrite);
        }
        pending.file.write_all(chunk)?;
        pending.digest.update(chunk);
        pending.written = next;
        Ok(())
    }

    fn finish_inactive_image(&mut self, slot: Slot) -> Result<(), Self::Error> {
        let pending = self.pending.take().ok_or(Error::InvalidImageWrite)?;
        if pending.slot != slot || pending.written != pending.expected_size {
            self.pending = Some(pending);
            return Err(Error::InvalidImageWrite);
        }
        if let Err(error) = pending
            .file
            .set_permissions(fs::Permissions::from_mode(0o755))
        {
            discard_temp_file(&pending.temporary);
            return Err(Error::Io(error));
        }
        if let Err(error) = pending.file.sync_all() {
            discard_temp_file(&pending.temporary);
            return Err(Error::Io(error));
        }
        let digest = pending.digest.finalize();
        drop(pending.file);
        if let Err(error) = fs::rename(&pending.temporary, &pending.destination) {
            discard_temp_file(&pending.temporary);
            return Err(Error::Io(error));
        }
        sync_directory(&pending.directory)?;
        write_image_digest_in_directory(&pending.directory, &digest)?;
        Ok(())
    }

    fn abort_inactive_image(&mut self, slot: Slot) {
        if let Some(pending) = self.pending.take() {
            if pending.slot == slot {
                drop(pending.file);
                let _ = fs::remove_file(pending.temporary);
                self.update_lock = None;
            } else {
                self.pending = Some(pending);
            }
        } else {
            self.update_lock = None;
        }
    }

    fn read_image_chunk(
        &self,
        slot: Slot,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, Self::Error> {
        validate_slot_image(&self.root, slot)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(self.image_path(slot))?;
        file.seek(SeekFrom::Start(u64::from(offset)))?;
        Ok(file.read(buffer)?)
    }
}

fn acquire_update_lock(root: &Path) -> Result<File, Error> {
    ensure_secure_directory(root, true)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(root.join(".fota-update.lock"))?;
    validate_owned_file(&file)?;
    // SAFETY: `file` owns a valid descriptor; flock does not retain the descriptor pointer.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        Ok(file)
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Err(Error::UpdateInProgress)
        } else {
            Err(Error::Io(error))
        }
    }
}

fn discard_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// The root launcher owns the socket pathname and passes an already-open listener to the
/// selected image. The application UID cannot replace this socket with a fake server.
struct RootHoldSocket {
    _listener: UnixListener,
    child_fd: OwnedFd,
    path: PathBuf,
    identity: (u64, u64),
}

impl RootHoldSocket {
    fn from_environment() -> Result<Option<Self>, Error> {
        let Some(path) = std::env::var_os(HOLD_SOCKET_ENV).map(PathBuf::from) else {
            return Ok(None);
        };
        Ok(Some(Self::bind(path)?))
    }

    fn bind(path: PathBuf) -> Result<Self, Error> {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(Error::UnsafeStatePath);
        }
        let parent = path.parent().ok_or(Error::UnsafeStatePath)?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        if !parent_metadata.file_type().is_dir()
            || parent_metadata.uid() != effective_uid()
            || parent_metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
        {
            return Err(Error::UnsafeStatePath);
        }
        let listener = UnixListener::bind(&path)?;
        let initial = fs::symlink_metadata(&path)?;
        if !initial.file_type().is_socket() || initial.uid() != effective_uid() {
            return Err(Error::UnsafeStatePath);
        }
        // Keep the inherited listener away from descriptors used internally by Command::spawn.
        // SAFETY: fcntl duplicates a valid listening socket descriptor; ownership is transferred
        // to `OwnedFd` below.
        let child_fd =
            unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_DUPFD_CLOEXEC, HOLD_FD_MINIMUM) };
        if child_fd == -1 {
            let identity = (initial.dev(), initial.ino());
            drop(listener);
            remove_owned_socket_path(&path, identity);
            return Err(Error::Io(io::Error::last_os_error()));
        }
        // SAFETY: `fcntl(F_DUPFD_CLOEXEC)` returned a fresh owned descriptor.
        let child_fd = unsafe { OwnedFd::from_raw_fd(child_fd) };
        let mut hold_socket = Self {
            _listener: listener,
            child_fd,
            path,
            identity: (initial.dev(), initial.ino()),
        };
        fs::set_permissions(&hold_socket.path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&hold_socket.path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != effective_uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(Error::UnsafeStatePath);
        }
        hold_socket.identity = (metadata.dev(), metadata.ino());
        Ok(hold_socket)
    }

    fn pass_to(&self, command: &mut Command) {
        let listener_fd = self.child_fd.as_raw_fd();
        command.env(HOLD_LISTENER_FD_ENV, listener_fd.to_string());
        // SAFETY: the child clears close-on-exec on the already-open root-owned listener so the
        // selected application can accept on the inherited endpoint. It does not overwrite any
        // descriptor that Command uses for its exec-error reporting pipe.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(listener_fd, libc::F_GETFD);
                if flags == -1
                    || libc::fcntl(listener_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

impl Drop for RootHoldSocket {
    fn drop(&mut self) {
        remove_owned_socket_path(&self.path, self.identity);
    }
}

fn remove_owned_socket_path(path: &Path, identity: (u64, u64)) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket()
            && metadata.uid() == effective_uid()
            && (metadata.dev(), metadata.ino()) == identity
        {
            let _ = fs::remove_file(path);
        }
    }
}

/// Select and run the configured A/B image. A trial image is committed only after the supplied
/// health command exits successfully before the manifest's trial window expires.
pub fn run_selected_image(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    run_selected_image_with_identity(root.into(), health_program, health_args, &[], None, false)
}

/// Select and run the configured A/B image with fixed application arguments.
///
/// Arguments are passed as individual operating-system strings to the selected image. They are
/// never interpreted by a shell and do not select the executable; the executable always comes
/// from the integrity-verified slot image.
pub fn run_selected_image_with_app_args(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    run_selected_image_with_identity(
        root.into(),
        health_program,
        health_args,
        app_args,
        None,
        false,
    )
}

/// Select and run an image and its health command under the configured unprivileged identity.
///
/// The launcher must run with permission to switch to `uid` and `gid` (normally through a
/// systemd unit with only CAP_SETUID and CAP_SETGID). Update metadata and image files remain owned
/// by the launcher account, so the application process cannot replace its own trusted image.
pub fn run_selected_image_as(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    uid: u32,
    gid: u32,
) -> Result<ExitStatus, Error> {
    run_selected_image_as_with_groups(root, health_program, health_args, uid, gid, &[])
}

/// Select and run an image with explicit supplementary groups for hardware access.
///
/// A Raspberry Pi application may need groups such as `i2c` and `gpio` to control a PiCar-X
/// Robot HAT while still running without root privileges.
pub fn run_selected_image_as_with_groups(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
) -> Result<ExitStatus, Error> {
    run_selected_image_as_with_groups_and_app_args(
        root,
        health_program,
        health_args,
        uid,
        gid,
        supplementary_groups,
        &[],
    )
}

/// Select and run the configured A/B image with fixed arguments and explicit supplementary groups.
///
/// The image arguments are passed directly to the integrity-verified slot executable via
/// `Command::args`; no shell command string is constructed.
pub fn run_selected_image_as_with_groups_and_app_args(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    if uid == 0 || gid == 0 || supplementary_groups.contains(&0) {
        return Err(Error::InvalidRunIdentity);
    }
    run_selected_image_with_identity(
        root.into(),
        health_program,
        health_args,
        app_args,
        Some((uid, gid, supplementary_groups.to_vec())),
        false,
    )
}

/// Resolve one fixed ECU A/B state root below the configured base directory.
///
/// The vehicle target uses the base directory; each virtual ECU uses its own fixed subdirectory.
/// This mapping is intentionally derived from the closed `Ecu` enum rather than a caller-supplied
/// path component.
pub fn state_root_for_target(base_root: impl Into<PathBuf>, target: crate::Ecu) -> PathBuf {
    let base_root = base_root.into();
    match target {
        crate::Ecu::Vehicle => base_root,
        crate::Ecu::Engine => base_root.join("ecus").join("engine"),
        crate::Ecu::Brake => base_root.join("ecus").join("brake"),
        crate::Ecu::Steering => base_root.join("ecus").join("steering"),
    }
}

/// Select and run one target's A/B image with fixed application arguments.
///
/// This trial-aware API is restricted to the vehicle target. ECU targets must use one of the
/// explicitly named active-only launch APIs so a service start cannot consume a pending trial.
pub fn run_selected_target_image_with_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    if target != crate::Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    let root = validate_target_root(base_root.into(), target, app_args)?;
    run_selected_image_with_identity(root, health_program, health_args, app_args, None, false)
}

/// Launch one target's selected A/B image as the configured unprivileged account.
///
/// ECU targets require exactly one `--ecu <target>` pair in `app_args`. They clear inherited
/// supplementary groups before exec, even when `supplementary_groups` is empty.
pub fn run_selected_target_image_as_with_groups_and_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    if target != crate::Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    if uid == 0 || gid == 0 || supplementary_groups.contains(&0) {
        return Err(Error::InvalidRunIdentity);
    }
    let root = validate_target_root(base_root.into(), target, app_args)?;
    run_selected_image_with_identity(
        root,
        health_program,
        health_args,
        app_args,
        Some((uid, gid, supplementary_groups.to_vec())),
        false,
    )
}

/// Launch an ECU target's current active image without selecting, committing, or rolling back a trial.
///
/// This variant runs under the caller's current identity. It validates the fixed ECU state root,
/// image integrity, and matching `--ecu <target>` application argument pair, then removes trial
/// environment variables before starting the active image.
pub fn run_active_target_image_with_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    if target == crate::Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    let root = validate_target_root(base_root.into(), target, app_args)?;
    run_active_image_with_identity(root, app_args, None, false)
}

/// Launch an ECU target's active image under a configured identity, without trial transitions.
///
/// This is the service-facing ECU launcher API. It leaves pending trial metadata byte-for-byte
/// unchanged and clears inherited supplementary groups even when `supplementary_groups` is empty.
pub fn run_active_target_image_as_with_groups_and_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
    app_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    if target == crate::Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    if uid == 0 || gid == 0 || supplementary_groups.contains(&0) {
        return Err(Error::InvalidRunIdentity);
    }
    let root = validate_target_root(base_root.into(), target, app_args)?;
    run_active_image_with_identity(
        root,
        app_args,
        Some((uid, gid, supplementary_groups.to_vec())),
        true,
    )
}

/// Launch one ECU's pending trial image as a shadow candidate, without changing boot metadata.
///
/// The candidate receives its own worker socket through fixed service arguments. It is not the
/// active worker route: the Pi coordinator may send it cycle inputs for validation, but must
/// discard its outputs until a later hold-and-cutover protocol explicitly promotes it. This API
/// never decrements `boots_left`, commits, or rolls back the trial.
pub fn run_trial_candidate_target_image_as_with_groups_and_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
    app_args: &[std::ffi::OsString],
    hold_socket: &Path,
) -> Result<ExitStatus, Error> {
    if uid == 0 || gid == 0 || supplementary_groups.contains(&0) {
        return Err(Error::InvalidRunIdentity);
    }
    run_trial_candidate_image(
        base_root.into(),
        target,
        Some((uid, gid, supplementary_groups.to_vec())),
        true,
        app_args,
        hold_socket,
    )
}

/// Launch a pending ECU image as a shadow candidate using the caller's current identity.
/// Service managers should use the identity-switching variant above.
pub fn run_trial_candidate_target_image_with_app_args(
    base_root: impl Into<PathBuf>,
    target: crate::Ecu,
    app_args: &[std::ffi::OsString],
    hold_socket: &Path,
) -> Result<ExitStatus, Error> {
    run_trial_candidate_image(base_root.into(), target, None, false, app_args, hold_socket)
}

fn run_trial_candidate_image(
    base_root: PathBuf,
    target: crate::Ecu,
    identity: Option<(u32, u32, Vec<u32>)>,
    clear_inherited_groups: bool,
    app_args: &[std::ffi::OsString],
    hold_socket: &Path,
) -> Result<ExitStatus, Error> {
    if target == crate::Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    let root = validate_target_root(base_root, target, app_args)?;
    // Match installer lock order: an in-flight image write makes candidate start fail closed.
    // The update lock protects image verification and exec startup. It is released once the new
    // process has been spawned so a long-lived shadow worker cannot block later package installs.
    let _update_lock = acquire_update_lock(&root)?;
    let _boot_lock = BootRecordLock::acquire(&root)?;
    let (_, state) = FileBootControl::new(&root).load()?;
    let trial = state.trial.ok_or(Error::NoTrialPending)?;
    if trial.slot == state.active || trial.boots_left == 0 {
        return Err(Error::NoTrialPending);
    }
    let storage = FileSlotStorage::new(&root);
    storage.verify_executable_image(trial.slot)?;
    let image = storage.image_path(trial.slot);
    request_output_hold(hold_socket, effective_uid())?;
    let mut command = command_with_identity(&image, identity.as_ref(), clear_inherited_groups);
    command
        .args(app_args)
        .env_remove("SDV_TRIAL_SLOT")
        .env_remove(HOLD_LISTENER_FD_ENV)
        .env_remove("SDV_FOTA_PENDING_ECU_TRIAL")
        .env("SDV_TRIAL_CANDIDATE_SLOT", slot_name(trial.slot))
        .env("SDV_IMAGE_PATH", &image);
    let mut child = command.spawn().map_err(Error::Io)?;
    drop(_boot_lock);
    drop(_update_lock);
    child.wait().map_err(Error::Io)
}

fn request_output_hold(path: &Path, expected_server_uid: u32) -> Result<(), Error> {
    let deadline = Instant::now() + OUTPUT_HOLD_CONNECT_TIMEOUT;
    let (mut stream, identity) = loop {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(error) => return Err(Error::Io(error)),
        };
        if !metadata.file_type().is_socket()
            || metadata.uid() != expected_server_uid
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(Error::OutputHoldFailed);
        }
        match UnixStream::connect(path) {
            Ok(stream) => break (stream, (metadata.dev(), metadata.ino())),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(Error::Io(error)),
        }
    };
    let current = fs::symlink_metadata(path).map_err(Error::Io)?;
    if !current.file_type().is_socket()
        || current.uid() != expected_server_uid
        || current.permissions().mode() & 0o777 != 0o600
        || (current.dev(), current.ino()) != identity
        || peer_uid(stream.as_raw_fd()).map_err(Error::Io)? != expected_server_uid
    {
        return Err(Error::OutputHoldFailed);
    }
    stream
        .set_read_timeout(Some(OUTPUT_HOLD_RESPONSE_TIMEOUT))
        .map_err(Error::Io)?;
    stream
        .set_write_timeout(Some(OUTPUT_HOLD_RESPONSE_TIMEOUT))
        .map_err(Error::Io)?;
    stream.write_all(OUTPUT_HOLD_REQUEST).map_err(Error::Io)?;
    stream.shutdown(Shutdown::Write).map_err(Error::Io)?;
    let mut response = Vec::with_capacity(OUTPUT_HOLD_NOT_READY.len());
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).map_err(Error::Io)?;
        response.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if response.len() >= OUTPUT_HOLD_NOT_READY.len() {
            return Err(Error::OutputHoldFailed);
        }
    }
    let mut trailing = [0u8; 1];
    match stream.read(&mut trailing) {
        Ok(0) if response.as_slice() == OUTPUT_HOLD_ACK => Ok(()),
        Ok(0) if response.as_slice() == OUTPUT_HOLD_NOT_READY => Err(Error::OutputHoldNotReady),
        Ok(_) => Err(Error::OutputHoldFailed),
        Err(error) => Err(Error::Io(error)),
    }
}

/// Request a one-way route cutover for an ECU candidate through the root-owned host control
/// socket. The vehicle process keeps its sticky zero-output hold latched after this call.
pub fn request_ecu_candidate_promotion(path: &Path, target: Ecu) -> Result<(), Error> {
    request_ecu_candidate_route_change(path, target, OUTPUT_PROMOTION_REQUEST_PREFIX, "SDVPROMOTED")
}

/// Return a promoted ECU route to its active worker through the root-owned host control socket.
/// The vehicle confirms a held zero-output cycle on the active route before acknowledging.
pub fn request_ecu_candidate_reversion(path: &Path, target: Ecu) -> Result<(), Error> {
    request_ecu_candidate_route_change(path, target, OUTPUT_REVERSION_REQUEST_PREFIX, "SDVREVERTED")
}

fn request_ecu_candidate_route_change(
    path: &Path,
    target: Ecu,
    request_prefix: &[u8],
    expected_response_prefix: &str,
) -> Result<(), Error> {
    if target == Ecu::Vehicle {
        return Err(Error::InvalidTargetLaunchMode);
    }
    let metadata = fs::symlink_metadata(path).map_err(Error::Io)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(Error::OutputHoldFailed);
    }
    let mut stream = UnixStream::connect(path).map_err(Error::Io)?;
    let current = fs::symlink_metadata(path).map_err(Error::Io)?;
    if !current.file_type().is_socket()
        || current.uid() != 0
        || current.permissions().mode() & 0o777 != 0o600
        || (current.dev(), current.ino()) != (metadata.dev(), metadata.ino())
        || peer_uid(stream.as_raw_fd()).map_err(Error::Io)? != 0
    {
        return Err(Error::OutputHoldFailed);
    }
    stream
        .set_read_timeout(Some(OUTPUT_HOLD_RESPONSE_TIMEOUT))
        .map_err(Error::Io)?;
    stream
        .set_write_timeout(Some(OUTPUT_HOLD_RESPONSE_TIMEOUT))
        .map_err(Error::Io)?;
    let target_name = match target {
        Ecu::Engine => "engine",
        Ecu::Brake => "brake",
        Ecu::Steering => "steering",
        Ecu::Vehicle => return Err(Error::InvalidTargetLaunchMode),
    };
    let mut request = request_prefix.to_vec();
    request.extend_from_slice(target_name.as_bytes());
    request.push(b'\n');
    stream.write_all(&request).map_err(Error::Io)?;
    stream.shutdown(Shutdown::Write).map_err(Error::Io)?;

    let mut response = Vec::with_capacity(64);
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).map_err(Error::Io)?;
        response.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if response.len() >= 255 {
            return Err(Error::OutputHoldFailed);
        }
    }
    let mut trailing = [0u8; 1];
    if stream.read(&mut trailing).map_err(Error::Io)? != 0 {
        return Err(Error::OutputHoldFailed);
    }
    if response == format!("{expected_response_prefix} {target_name}\n").as_bytes() {
        Ok(())
    } else if response.as_slice() == OUTPUT_HOLD_NOT_READY {
        Err(Error::OutputHoldNotReady)
    } else if response.starts_with(b"SDVROUTE_FAILED ") {
        Err(Error::CandidateRouteChangeFailed)
    } else {
        Err(Error::OutputHoldFailed)
    }
}

fn peer_uid(fd: i32) -> io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` is writable and the supplied length is its exact structure size.
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

fn validate_target_root(
    base_root: PathBuf,
    target: crate::Ecu,
    app_args: &[std::ffi::OsString],
) -> Result<PathBuf, Error> {
    if !target_app_args_match(target, app_args) {
        return Err(Error::InvalidTargetArguments);
    }
    if target != crate::Ecu::Vehicle {
        // ECU roots share this fixed, root-provisioned ancestor. Reject an ancestor symlink so
        // selecting one ECU cannot redirect its boot state into another target's tree.
        ensure_secure_directory(&base_root.join("ecus"), false)?;
    }
    Ok(state_root_for_target(base_root, target))
}

fn target_app_args_match(target: crate::Ecu, app_args: &[std::ffi::OsString]) -> bool {
    let expected = match target {
        crate::Ecu::Vehicle => return true,
        crate::Ecu::Engine => "engine",
        crate::Ecu::Brake => "brake",
        crate::Ecu::Steering => "steering",
    };
    let mut found = false;
    let mut index = 0;
    while index < app_args.len() {
        let Some(option) = app_args[index].to_str() else {
            index += 1;
            continue;
        };
        match option {
            "--ecu" => {
                if found
                    || app_args.get(index + 1).and_then(|value| value.to_str()) != Some(expected)
                {
                    return false;
                }
                found = true;
                index += 2;
            }
            "--worker-socket" | "--coordinator-socket" => {
                if app_args.get(index + 1).is_none() {
                    return false;
                }
                index += 2;
            }
            _ if option.starts_with("--") => return false,
            _ => index += 1,
        }
    }
    found
}

fn command_with_identity(
    program: &Path,
    identity: Option<&(u32, u32, Vec<u32>)>,
    clear_inherited_groups: bool,
) -> Command {
    let mut command = Command::new(program);
    if let Some((uid, gid, groups)) = identity {
        let uid = *uid;
        let gid = *gid;
        let groups = groups.clone();
        // SAFETY: the child applies its credentials immediately before exec and performs no
        // allocation or locking in this callback. The groups buffer is owned by the closure.
        unsafe {
            command.pre_exec(move || {
                let should_set_groups = !groups.is_empty() || clear_inherited_groups;
                let group_list = if groups.is_empty() {
                    std::ptr::null()
                } else {
                    groups.as_ptr()
                };
                if (should_set_groups && setgroups(groups.len(), group_list) != 0)
                    || setgid(gid) != 0
                    || setuid(uid) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
}

fn run_selected_image_with_identity(
    root: PathBuf,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    app_args: &[std::ffi::OsString],
    identity: Option<(u32, u32, Vec<u32>)>,
    clear_inherited_groups: bool,
) -> Result<ExitStatus, Error> {
    let hold_socket = RootHoldSocket::from_environment()?;
    let ecu_trial_pending = has_pending_ecu_trial(&root)?;
    let mut boot = FileBootControl::new(&root);
    let decision = boot.select_for_boot()?;
    let storage = FileSlotStorage::new(&root);
    if let Err(error) = storage.verify_executable_image(decision.slot) {
        if decision.trial {
            boot.rollback_trial()?;
        }
        return Err(error);
    }
    let image = storage.image_path(decision.slot);
    let trial_window_ms = if decision.trial {
        boot.state()?
            .trial
            .ok_or(Error::NoTrialPending)?
            .trial_window_ms
    } else {
        0
    };
    let mut image_command =
        command_with_identity(&image, identity.as_ref(), clear_inherited_groups);
    match hold_socket.as_ref() {
        Some(hold_socket) => hold_socket.pass_to(&mut image_command),
        None => {
            image_command.env_remove(HOLD_LISTENER_FD_ENV);
        }
    }
    image_command.args(app_args);
    if ecu_trial_pending {
        image_command.env("SDV_FOTA_PENDING_ECU_TRIAL", "1");
    } else {
        image_command.env_remove("SDV_FOTA_PENDING_ECU_TRIAL");
    }
    if decision.trial {
        image_command
            .env("SDV_TRIAL_SLOT", slot_name(decision.slot))
            .env("SDV_IMAGE_PATH", &image);
    } else {
        image_command
            .env_remove("SDV_TRIAL_SLOT")
            .env_remove("SDV_IMAGE_PATH");
    }
    let mut child = match image_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if decision.trial {
                boot.rollback_trial()?;
            }
            return Err(Error::Io(error));
        }
    };

    if decision.trial {
        if let Err(error) = wait_for_trial_health(
            &mut child,
            &mut boot,
            decision.slot,
            &image,
            health_program,
            health_args,
            Duration::from_millis(u64::from(trial_window_ms)),
            identity.as_ref(),
            clear_inherited_groups,
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = boot.commit_trial() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        stop_trial_process_for_restart(&mut child)?;
        // Report the successful commit to callers. The systemd unit uses Restart=always, so its
        // next launch starts the committed image without the trial environment.
        return Ok(ExitStatus::from_raw(0));
    }
    child.wait().map_err(Error::Io)
}

fn has_pending_ecu_trial(base_root: &Path) -> Result<bool, Error> {
    let ecus_root = base_root.join("ecus");
    let metadata = match fs::symlink_metadata(&ecus_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::Io(error)),
    };
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
    {
        return Err(Error::UnsafeStatePath);
    }

    for target in [crate::Ecu::Engine, crate::Ecu::Brake, crate::Ecu::Steering] {
        let ecu_root = state_root_for_target(base_root, target);
        match fs::symlink_metadata(&ecu_root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Error::Io(error)),
            Ok(metadata)
                if metadata.file_type().is_dir()
                    && metadata.uid() == effective_uid()
                    && metadata.permissions().mode() & UNSAFE_WRITE_BITS == 0 => {}
            Ok(_) => return Err(Error::UnsafeStatePath),
        }
        let (_, state) = FileBootControl::new(&ecu_root).load()?;
        if state.trial.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn run_active_image_with_identity(
    root: PathBuf,
    app_args: &[std::ffi::OsString],
    identity: Option<(u32, u32, Vec<u32>)>,
    clear_inherited_groups: bool,
) -> Result<ExitStatus, Error> {
    let boot = FileBootControl::new(&root);
    let state = boot.state()?;
    let storage = FileSlotStorage::new(&root);
    storage.verify_executable_image(state.active)?;
    let image = storage.image_path(state.active);
    let mut command = command_with_identity(&image, identity.as_ref(), clear_inherited_groups);
    command
        .args(app_args)
        .env_remove("SDV_TRIAL_SLOT")
        .env_remove(HOLD_LISTENER_FD_ENV)
        .env_remove("SDV_FOTA_PENDING_ECU_TRIAL")
        .env_remove("SDV_IMAGE_PATH");
    let mut child = command.spawn().map_err(Error::Io)?;
    child.wait().map_err(Error::Io)
}

fn stop_trial_process_for_restart(child: &mut Child) -> Result<ExitStatus, Error> {
    let process_id = libc::pid_t::try_from(child.id()).map_err(|_| Error::InvalidRunIdentity)?;
    // The PiCar-X app catches SIGTERM and stops its actuators before systemd starts the committed
    // image again without the trial environment.
    if unsafe { libc::kill(process_id, libc::SIGTERM) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(Error::Io(error));
        }
    }
    let shutdown_deadline = Instant::now() + TRIAL_PROCESS_STOP_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(Error::Io)? {
            return Ok(status);
        }
        if Instant::now() >= shutdown_deadline {
            child.kill().map_err(Error::Io)?;
            return child.wait().map_err(Error::Io);
        }
        thread::sleep(PROCESS_EXIT_POLL_INTERVAL);
    }
}

fn wait_for_trial_health(
    child: &mut Child,
    boot: &mut FileBootControl,
    slot: Slot,
    image: &Path,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
    timeout: Duration,
    identity: Option<&(u32, u32, Vec<u32>)>,
    clear_inherited_groups: bool,
) -> Result<(), Error> {
    let started = Instant::now();
    let stability_window = TRIAL_HEALTH_STABILITY_WINDOW.min(timeout / 2);
    if stability_window.is_zero() {
        boot.rollback_trial()?;
        return Err(Error::TrialHealthTimeout);
    }
    let poll_interval = TRIAL_HEALTH_POLL_INTERVAL
        .min(stability_window / 4)
        .max(Duration::from_millis(1));
    let mut healthy_since = None;
    loop {
        if started.elapsed() >= timeout {
            boot.rollback_trial()?;
            return Err(Error::TrialHealthTimeout);
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                boot.rollback_trial()?;
                return Err(Error::TrialImageExited);
            }
            Ok(None) => {}
            Err(error) => {
                boot.rollback_trial()?;
                return Err(Error::Io(error));
            }
        }
        let mut health_command =
            command_with_identity(health_program, identity, clear_inherited_groups);
        health_command
            .args(health_args)
            .env("SDV_TRIAL_SLOT", slot_name(slot))
            .env("SDV_IMAGE_PATH", image);
        let mut health = match health_command.spawn() {
            Ok(child) => child,
            Err(error) => {
                boot.rollback_trial()?;
                return Err(Error::Io(error));
            }
        };
        let mut health_succeeded = false;
        loop {
            match health.try_wait() {
                Ok(Some(status)) if status.success() => {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            boot.rollback_trial()?;
                            return Err(Error::TrialImageExited);
                        }
                        Ok(None) => health_succeeded = true,
                        Err(error) => {
                            boot.rollback_trial()?;
                            return Err(Error::Io(error));
                        }
                    }
                    break;
                }
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => {
                    let _ = health.kill();
                    let _ = health.wait();
                    boot.rollback_trial()?;
                    return Err(Error::Io(error));
                }
            }
            match child.try_wait() {
                Ok(Some(_)) => {
                    let _ = health.kill();
                    let _ = health.wait();
                    boot.rollback_trial()?;
                    return Err(Error::TrialImageExited);
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = health.kill();
                    let _ = health.wait();
                    boot.rollback_trial()?;
                    return Err(Error::Io(error));
                }
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                let _ = health.kill();
                let _ = health.wait();
                boot.rollback_trial()?;
                return Err(Error::TrialHealthTimeout);
            }
            thread::sleep(
                Duration::from_millis(10)
                    .min(poll_interval)
                    .min(timeout - elapsed),
            );
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            boot.rollback_trial()?;
            return Err(Error::TrialHealthTimeout);
        }
        if health_succeeded {
            let since = healthy_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= stability_window {
                return Ok(());
            }
        } else {
            healthy_since = None;
        }
        thread::sleep(poll_interval.min(timeout - elapsed));
    }
}

/// Redundant, CRC-protected boot metadata stored under the application-data directory.
pub struct FileBootControl {
    root: PathBuf,
}

impl FileBootControl {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Record the slot installed during provisioning. Refuses to overwrite existing state.
    pub fn initialize(
        &self,
        active: Slot,
        active_version: &str,
        highest_security_version: u64,
    ) -> Result<(), Error> {
        self.initialize_inner(None, active, active_version, highest_security_version)
    }

    /// Initialize boot metadata and the active image digest under one boot-record lock.
    pub fn initialize_with_image(
        &self,
        storage: &FileSlotStorage,
        active: Slot,
        active_version: &str,
        highest_security_version: u64,
    ) -> Result<(), Error> {
        if storage.root != self.root {
            return Err(Error::UnsafeStatePath);
        }
        self.initialize_inner(
            Some(storage),
            active,
            active_version,
            highest_security_version,
        )
    }

    /// Add an integrity record for an existing active image with no pending trial.
    pub fn record_active_image_integrity(
        &self,
        storage: &FileSlotStorage,
        slot: Slot,
    ) -> Result<(), Error> {
        if storage.root != self.root {
            return Err(Error::UnsafeStatePath);
        }
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (_, state) = self.load()?;
        if state.trial.is_some() {
            return Err(Error::TrialAlreadyPending);
        }
        if state.active != slot {
            return Err(Error::SlotIsNotActive);
        }
        storage.record_image_integrity(slot)
    }

    fn initialize_inner(
        &self,
        storage: Option<&FileSlotStorage>,
        active: Slot,
        active_version: &str,
        highest_security_version: u64,
    ) -> Result<(), Error> {
        ensure_secure_directory(&self.root, true)?;
        let _lock = BootRecordLock::acquire(&self.root)?;
        if path_entry_exists(&self.root.join("bootctl.a"))
            || path_entry_exists(&self.root.join("bootctl.b"))
        {
            return Err(Error::AlreadyInitialized);
        }
        if let Some(storage) = storage {
            storage.record_image_integrity(active)?;
        }
        self.save(
            0,
            &BootState {
                active,
                active_version: VersionString::try_from_str(active_version)
                    .ok_or(Error::InvalidRecord)?,
                highest_security_version,
                trial: None,
                last_trial: None,
            },
        )
    }

    fn load(&self) -> Result<(u64, BootState), Error> {
        ensure_secure_directory(&self.root, false)?;
        let a = read_record(&self.root.join("bootctl.a"));
        let b = read_record(&self.root.join("bootctl.b"));
        match (a, b) {
            (Some(a), Some(b)) => Ok(if a.0 >= b.0 { a } else { b }),
            (Some(a), None) | (None, Some(a)) => Ok(a),
            (None, None) => Err(Error::NoValidBootControl),
        }
    }

    fn save(&self, sequence: u64, state: &BootState) -> Result<(), Error> {
        ensure_secure_directory(&self.root, false)?;
        let bytes = encode_record(sequence, state)?;
        let name = if sequence % 2 == 0 {
            "bootctl.a"
        } else {
            "bootctl.b"
        };
        let destination = self.root.join(name);
        let (temporary, mut file) = create_temporary_file(&self.root, name)?;
        let write_result = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            discard_temp_file(&temporary);
            return Err(Error::Io(error));
        }
        if let Err(error) = fs::rename(&temporary, destination) {
            discard_temp_file(&temporary);
            return Err(Error::Io(error));
        }
        sync_directory(&self.root)?;
        Ok(())
    }
}

impl BootControl for FileBootControl {
    type Error = Error;

    fn state(&self) -> Result<BootState, Self::Error> {
        self.load().map(|(_, state)| state)
    }

    fn schedule_trial(&mut self, trial: TrialImage) -> Result<(), Self::Error> {
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (sequence, mut state) = self.load()?;
        if state.trial.is_some() {
            return Err(Error::TrialAlreadyPending);
        }
        if trial.slot == state.active || trial.boots_left == 0 {
            return Err(Error::InvalidRecord);
        }
        state.trial = Some(trial);
        self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)
    }

    fn select_for_boot(&mut self) -> Result<BootDecision, Self::Error> {
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (sequence, mut state) = self.load()?;
        let Some(mut trial) = state.trial else {
            return Ok(BootDecision {
                slot: state.active,
                trial: false,
            });
        };
        if trial.boots_left == 0 {
            state.trial = None;
            state.last_trial = Some(LastTrialResult::from_trial(trial, TrialOutcome::RolledBack));
            self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)?;
            return Ok(BootDecision {
                slot: state.active,
                trial: false,
            });
        }
        trial.boots_left -= 1;
        let slot = trial.slot;
        state.trial = Some(trial);
        self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)?;
        Ok(BootDecision { slot, trial: true })
    }

    fn commit_trial(&mut self) -> Result<(), Self::Error> {
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (sequence, mut state) = self.load()?;
        let trial = state.trial.take().ok_or(Error::NoTrialPending)?;
        state.active = trial.slot;
        state.active_version = trial.version;
        state.highest_security_version = state.highest_security_version.max(trial.security_version);
        state.last_trial = Some(LastTrialResult::from_trial(trial, TrialOutcome::Committed));
        self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)
    }

    fn rollback_trial(&mut self) -> Result<(), Self::Error> {
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (sequence, mut state) = self.load()?;
        let trial = state.trial.take().ok_or(Error::NoTrialPending)?;
        state.last_trial = Some(LastTrialResult::from_trial(trial, TrialOutcome::RolledBack));
        self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)
    }
}

fn encode_record(sequence: u64, state: &impl Serialize) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(state).map_err(|_| Error::InvalidRecord)?;
    let body_len = u32::try_from(body.len()).map_err(|_| Error::InvalidRecord)?;
    let mut bytes = Vec::with_capacity(RECORD_HEADER_BYTES + body.len() + 4);
    bytes.extend_from_slice(RECORD_MAGIC);
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&body_len.to_be_bytes());
    bytes.extend_from_slice(&body);
    let checksum = crc32(&bytes[4..]);
    bytes.extend_from_slice(&checksum.to_be_bytes());
    if bytes.len() > MAX_BOOT_RECORD_BYTES {
        return Err(Error::InvalidRecord);
    }
    Ok(bytes)
}

fn read_record(path: &Path) -> Option<(u64, BootState)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .ok()?;
    validate_owned_file(&file).ok()?;
    let size = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if !(RECORD_HEADER_BYTES + 4..=MAX_BOOT_RECORD_BYTES).contains(&size) {
        return None;
    }
    let mut bytes = vec![0u8; size];
    file.read_exact(&mut bytes).ok()?;
    let mut trailing = [0u8; 1];
    if file.read(&mut trailing).ok()? != 0 {
        return None;
    }
    if bytes.len() < RECORD_HEADER_BYTES + 4 || &bytes[..4] != RECORD_MAGIC {
        return None;
    }
    let sequence = u64::from_be_bytes(bytes[4..12].try_into().ok()?);
    let body_len = u32::from_be_bytes(bytes[12..16].try_into().ok()?) as usize;
    if RECORD_HEADER_BYTES.checked_add(body_len)?.checked_add(4)? != bytes.len() {
        return None;
    }
    let checksum = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().ok()?);
    if checksum != crc32(&bytes[4..bytes.len() - 4]) {
        return None;
    }
    let state = serde_json::from_slice(&bytes[RECORD_HEADER_BYTES..bytes.len() - 4]).ok()?;
    Some((sequence, state))
}

fn ensure_secure_directory(path: &Path, create: bool) -> Result<(), Error> {
    if create {
        fs::create_dir_all(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
    {
        return Err(Error::UnsafeStatePath);
    }
    Ok(())
}

fn ensure_slot_directories(root: &Path, slot: Slot, create: bool) -> Result<(), Error> {
    ensure_secure_directory(&root.join("slots"), create)?;
    ensure_secure_directory(&root.join("slots").join(slot_name(slot)), create)
}

fn validate_slot_image(root: &Path, slot: Slot) -> Result<(), Error> {
    ensure_secure_directory(root, false)?;
    ensure_slot_directories(root, slot, false)?;
    let metadata = fs::symlink_metadata(root.join("slots").join(slot_name(slot)).join(IMAGE_FILE))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
    {
        return Err(Error::UnsafeStatePath);
    }
    Ok(())
}

fn open_slot_image(root: &Path, slot: Slot) -> Result<File, Error> {
    validate_slot_image(root, slot)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(root.join("slots").join(slot_name(slot)).join(IMAGE_FILE))?;
    validate_owned_file(&file)?;
    Ok(file)
}

fn hash_file(mut file: File) -> Result<[u8; 32], Error> {
    let mut digest = sdv_sec::Sha256Context::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(digest.finalize());
        }
        digest.update(&buffer[..read]);
    }
}

fn write_image_digest(root: &Path, slot: Slot, digest: &[u8; 32]) -> Result<(), Error> {
    ensure_secure_directory(root, false)?;
    ensure_slot_directories(root, slot, false)?;
    let directory = root.join("slots").join(slot_name(slot));
    write_image_digest_in_directory(&directory, digest)
}

fn write_image_digest_in_directory(directory: &Path, digest: &[u8; 32]) -> Result<(), Error> {
    let (temporary, mut file) = create_temporary_file(directory, "sdv-image-sha256")?;
    let mut encoded = [0u8; IMAGE_DIGEST_HEX_BYTES];
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, byte) in digest.iter().copied().enumerate() {
        encoded[index * 2] = HEX[(byte >> 4) as usize];
        encoded[index * 2 + 1] = HEX[(byte & 0x0f) as usize];
    }
    let write_result = file.write_all(&encoded).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        discard_temp_file(&temporary);
        return Err(Error::Io(error));
    }
    let destination = directory.join(IMAGE_DIGEST_FILE);
    if let Err(error) = fs::rename(&temporary, &destination) {
        discard_temp_file(&temporary);
        return Err(Error::Io(error));
    }
    sync_directory(directory)?;
    Ok(())
}

fn read_image_digest(root: &Path, slot: Slot) -> Result<[u8; 32], Error> {
    ensure_secure_directory(root, false)?;
    ensure_slot_directories(root, slot, false)?;
    let path = root
        .join("slots")
        .join(slot_name(slot))
        .join(IMAGE_DIGEST_FILE);
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)?;
    validate_owned_file(&file)?;
    if file.metadata()?.len() != IMAGE_DIGEST_HEX_BYTES as u64 {
        return Err(Error::InvalidBootImage);
    }
    let mut encoded = [0u8; IMAGE_DIGEST_HEX_BYTES];
    file.read_exact(&mut encoded)?;
    let encoded = core::str::from_utf8(&encoded).map_err(|_| Error::InvalidBootImage)?;
    decode_sha256(encoded).ok_or(Error::InvalidBootImage)
}

fn validate_owned_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & UNSAFE_WRITE_BITS != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "FOTA state file is not a protected regular file owned by this account",
        ));
    }
    Ok(())
}

fn create_temporary_file(directory: &Path, prefix: &str) -> Result<(PathBuf, File), Error> {
    for _ in 0..32 {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(".{prefix}.{}.{}.tmp", std::process::id(), sequence));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::Io(error)),
        }
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique FOTA temporary file",
    )))
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and returns the process effective user ID.
    unsafe { geteuid() }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn slot_name(slot: Slot) -> &'static str {
    match slot {
        Slot::A => "a",
        Slot::B => "b",
    }
}

#[cfg(test)]
mod output_hold_fd_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn root_hold_listener_is_inherited_by_exec_without_clobbering_command_pipe() {
        let path = std::env::temp_dir().join(format!(
            "sdv-fota-hold-fd-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = path.join("control.sock");
        let hold_socket = RootHoldSocket::bind(socket_path.clone()).unwrap();

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "test -S \"/proc/self/fd/$SDV_FOTA_HOLD_LISTENER_FD\""]);
        hold_socket.pass_to(&mut command);
        let status = command.status().unwrap();

        assert!(status.success());
        assert!(socket_path.exists());
        drop(hold_socket);
        assert!(!socket_path.exists());
        fs::remove_dir(path).unwrap();
    }
}
