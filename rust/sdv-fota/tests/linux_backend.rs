#![cfg(target_os = "linux")]

use sdv_fota::linux::{FileBootControl, FileSlotStorage};
use sdv_fota::{BootControl, ImageStorage, Slot, TrialImage};
use std::fs;
use std::path::PathBuf;
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

#[test]
fn linux_backend_stages_image_and_commits_trial() {
    let root = TempRoot::new();
    let mut images = FileSlotStorage::new(&root.0);
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0".into(), 1).unwrap();

    let slot = images
        .write_inactive_image(Slot::A, b"candidate image")
        .unwrap();
    assert_eq!(slot, Slot::B);
    assert_eq!(images.read_image(Slot::B).unwrap(), b"candidate image");
    boot.schedule_trial(TrialImage {
        slot,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 1,
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
