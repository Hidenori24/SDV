#![cfg(target_os = "linux")]

use sdv_fota::linux::{run_selected_image, FileBootControl, FileSlotStorage};
use sdv_fota::{BootControl, ImageStorage, Slot, TrialImage};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sdv-fota-linux-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn linux_backend_stages_image_and_commits_trial() {
    let root = TempRoot::new();
    let mut images = FileSlotStorage::new(&root.0);
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();

    let slot = images
        .begin_inactive_image(Slot::A, b"candidate image".len() as u32)
        .unwrap();
    assert_eq!(slot, Slot::B);
    images.write_image_chunk(slot, 0, b"candidate").unwrap();
    images.write_image_chunk(slot, 9, b" image").unwrap();
    images.finish_inactive_image(slot).unwrap();
    let mut readback = [0u8; 8];
    let first = images.read_image_chunk(Slot::B, 0, &mut readback).unwrap();
    let mut full_image = readback[..first].to_vec();
    let second = images
        .read_image_chunk(Slot::B, first as u32, &mut readback)
        .unwrap();
    full_image.extend_from_slice(&readback[..second]);
    assert_eq!(full_image, b"candidate image");
    boot.schedule_trial(TrialImage {
        slot,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 1,
        trial_window_ms: 5_000,
    })
    .unwrap();
    assert_eq!(boot.select_for_boot().unwrap().slot, Slot::B);

    boot.commit_trial().unwrap();
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::B);
    assert_eq!(state.active_version, "1.1.0");
    assert_eq!(state.highest_security_version, 2);
    assert!(state.trial.is_none());
}

#[test]
fn corrupt_newest_boot_record_falls_back_to_previous_record() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 1,
        trial_window_ms: 5_000,
    })
    .unwrap();

    let newest_record = root.0.join("bootctl.b");
    let mut bytes = fs::read(&newest_record).unwrap();
    let body_byte = bytes.len() - 5;
    bytes[body_byte] ^= 0x80;
    fs::write(newest_record, bytes).unwrap();

    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert_eq!(state.active_version, "1.0.0");
    assert!(state.trial.is_none());
}

#[test]
fn linux_launcher_commits_trial_after_health_command_succeeds() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();
    let slot_dir = root.0.join("slots/b");
    fs::create_dir_all(&slot_dir).unwrap();
    write_executable(&slot_dir.join("sdv-image.bin"), "#!/bin/sh\nexec sleep 2\n");
    let health = root.0.join("health-check");
    write_executable(
        &health,
        "#!/bin/sh\ntest \"$SDV_TRIAL_SLOT\" = b && test -x \"$SDV_IMAGE_PATH\"\n",
    );
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 2_000,
    })
    .unwrap();

    let status = run_selected_image(&root.0, &health, &[]).unwrap();

    assert!(status.success());
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::B);
    assert_eq!(state.active_version, "1.1.0");
    assert!(state.trial.is_none());
}

#[test]
fn linux_launcher_rolls_back_trial_when_health_window_expires() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();
    let slot_dir = root.0.join("slots/b");
    fs::create_dir_all(&slot_dir).unwrap();
    write_executable(&slot_dir.join("sdv-image.bin"), "#!/bin/sh\nexec sleep 2\n");
    let health = root.0.join("health-check");
    write_executable(&health, "#!/bin/sh\nexit 1\n");
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 20,
    })
    .unwrap();

    let result = run_selected_image(&root.0, &health, &[]);

    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::TrialHealthTimeout)
    ));
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert_eq!(state.active_version, "1.0.0");
    assert!(state.trial.is_none());
}

#[test]
fn linux_launcher_rolls_back_if_trial_image_exits_before_health_check() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();
    let slot_dir = root.0.join("slots/b");
    fs::create_dir_all(&slot_dir).unwrap();
    write_executable(&slot_dir.join("sdv-image.bin"), "#!/bin/sh\nexit 1\n");
    let health = root.0.join("health-check");
    write_executable(&health, "#!/bin/sh\nexit 0\n");
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 2_000,
    })
    .unwrap();

    let result = run_selected_image(&root.0, &health, &[]);

    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::TrialImageExited)
    ));
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert!(state.trial.is_none());
}
