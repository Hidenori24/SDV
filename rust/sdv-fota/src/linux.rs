//! Raspberry Pi 5 Linux userspace adapters for A/B SDV application images.
//!
//! These adapters write only under the configured application-data directory. They do not
//! touch the running root filesystem, EEPROM, boot partition, or raw block devices. A systemd
//! launcher should call `select_for_boot`, start the returned slot's image, then call
//! `commit_trial` only after the SDV service passes its health checks.

use super::{BootControl, BootDecision, BootState, ImageStorage, Slot, TrialImage, VersionString};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const RECORD_MAGIC: &[u8; 4] = b"SDVB";
const RECORD_HEADER_BYTES: usize = 16;
const MAX_BOOT_RECORD_BYTES: usize = 4096;
const IMAGE_FILE: &str = "sdv-image.bin";
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const LOCK_UN: i32 = 8;

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
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
    TrialImageExited,
    TrialHealthTimeout,
    TrailingPackageData,
    UpdateInProgress,
    InvalidHttpsUrl,
    DownloadFailed,
    InvalidRunIdentity,
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
        let Some(host_and_path) = url.strip_prefix("https://") else {
            return Err(Error::InvalidHttpsUrl);
        };
        if host_and_path.is_empty() || host_and_path.starts_with('/') {
            return Err(Error::InvalidHttpsUrl);
        }
        let mut child = Command::new("curl")
            .args([
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
                "--",
                url,
            ])
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
}

/// Serializes read-modify-write boot metadata transitions across installer and launcher processes.
struct BootRecordLock(File);

impl BootRecordLock {
    fn acquire(root: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(root.join(".fota-bootctl.lock"))?;
        // SAFETY: this is a valid open file descriptor; flock is released when the guard drops.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
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
        let new_lock = if self.update_lock.is_none() {
            Some(acquire_update_lock(&self.root)?)
        } else {
            None
        };
        let slot = active.other();
        let directory = self.root.join("slots").join(slot_name(slot));
        fs::create_dir_all(&directory)?;
        let temporary = directory.join("sdv-image.tmp");
        let destination = directory.join(IMAGE_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        self.pending = Some(PendingImage {
            slot,
            directory,
            temporary,
            destination,
            file,
            expected_size: image_size,
            written: 0,
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
        drop(pending.file);
        if let Err(error) = fs::rename(&pending.temporary, &pending.destination) {
            discard_temp_file(&pending.temporary);
            return Err(Error::Io(error));
        }
        sync_directory(&pending.directory)?;
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
        let mut file = File::open(self.image_path(slot))?;
        file.seek(SeekFrom::Start(u64::from(offset)))?;
        Ok(file.read(buffer)?)
    }
}

fn acquire_update_lock(root: &Path) -> Result<File, Error> {
    fs::create_dir_all(root)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".fota-update.lock"))?;
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

/// Select and run the configured A/B image. A trial image is committed only after the supplied
/// health command exits successfully before the manifest's trial window expires.
pub fn run_selected_image(
    root: impl Into<PathBuf>,
    health_program: &Path,
    health_args: &[std::ffi::OsString],
) -> Result<ExitStatus, Error> {
    run_selected_image_with_identity(root.into(), health_program, health_args, None)
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
    if uid == 0 || gid == 0 || supplementary_groups.contains(&0) {
        return Err(Error::InvalidRunIdentity);
    }
    run_selected_image_with_identity(
        root.into(),
        health_program,
        health_args,
        Some((uid, gid, supplementary_groups.to_vec())),
    )
}

fn command_with_identity(program: &Path, identity: Option<&(u32, u32, Vec<u32>)>) -> Command {
    let mut command = Command::new(program);
    if let Some((uid, gid, groups)) = identity {
        let uid = *uid;
        let gid = *gid;
        let groups = groups.clone();
        // SAFETY: the child applies its credentials immediately before exec and performs no
        // allocation or locking in this callback. The groups buffer is owned by the closure.
        unsafe {
            command.pre_exec(move || {
                if (!groups.is_empty() && setgroups(groups.len(), groups.as_ptr()) != 0)
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
    identity: Option<(u32, u32, Vec<u32>)>,
) -> Result<ExitStatus, Error> {
    let mut boot = FileBootControl::new(&root);
    let decision = boot.select_for_boot()?;
    let image = FileSlotStorage::new(&root).image_path(decision.slot);
    let trial_window_ms = if decision.trial {
        boot.state()?
            .trial
            .ok_or(Error::NoTrialPending)?
            .trial_window_ms
    } else {
        0
    };
    let mut child = match command_with_identity(&image, identity.as_ref()).spawn() {
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
    }
    child.wait().map_err(Error::Io)
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
) -> Result<(), Error> {
    let started = Instant::now();
    loop {
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
        let mut health_command = command_with_identity(health_program, identity);
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
        loop {
            match health.try_wait() {
                Ok(Some(status)) if status.success() => match child.try_wait() {
                    Ok(Some(_)) => {
                        boot.rollback_trial()?;
                        return Err(Error::TrialImageExited);
                    }
                    Ok(None) => return Ok(()),
                    Err(error) => {
                        boot.rollback_trial()?;
                        return Err(Error::Io(error));
                    }
                },
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
            thread::sleep(Duration::from_millis(10).min(timeout - elapsed));
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            boot.rollback_trial()?;
            return Err(Error::TrialHealthTimeout);
        }
        thread::sleep(Duration::from_millis(100).min(timeout - elapsed));
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
        fs::create_dir_all(&self.root)?;
        let _lock = BootRecordLock::acquire(&self.root)?;
        if self.root.join("bootctl.a").exists() || self.root.join("bootctl.b").exists() {
            return Err(Error::AlreadyInitialized);
        }
        self.save(
            0,
            &BootState {
                active,
                active_version: VersionString::try_from_str(active_version)
                    .ok_or(Error::InvalidRecord)?,
                highest_security_version,
                trial: None,
            },
        )
    }

    fn load(&self) -> Result<(u64, BootState), Error> {
        let a = read_record(&self.root.join("bootctl.a"));
        let b = read_record(&self.root.join("bootctl.b"));
        match (a, b) {
            (Some(a), Some(b)) => Ok(if a.0 >= b.0 { a } else { b }),
            (Some(a), None) | (None, Some(a)) => Ok(a),
            (None, None) => Err(Error::NoValidBootControl),
        }
    }

    fn save(&self, sequence: u64, state: &BootState) -> Result<(), Error> {
        let bytes = encode_record(sequence, state)?;
        let name = if sequence % 2 == 0 {
            "bootctl.a"
        } else {
            "bootctl.b"
        };
        let destination = self.root.join(name);
        let temporary = self.root.join(format!("{name}.tmp"));
        fs::create_dir_all(&self.root)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, destination)?;
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
        self.save(sequence.checked_add(1).ok_or(Error::InvalidRecord)?, &state)
    }

    fn rollback_trial(&mut self) -> Result<(), Self::Error> {
        let _lock = BootRecordLock::acquire(&self.root)?;
        let (sequence, mut state) = self.load()?;
        if state.trial.take().is_none() {
            return Err(Error::NoTrialPending);
        }
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
    let mut file = File::open(path).ok()?;
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
