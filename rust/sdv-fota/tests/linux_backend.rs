#![cfg(target_os = "linux")]

use sdv_fota::linux::{run_selected_image, FileBootControl, FileSlotStorage};
use sdv_fota::{encode_package, BootControl, ImageStorage, Slot, TrialImage, VersionString};
use sdv_sec::{ed25519_public_key, ed25519_sign, sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
const SIGNING_SEED: [u8; 32] = [7; 32];
const KEY_ID: &str = "pi-test-key";

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fixed_version(version: &str) -> VersionString {
    VersionString::try_from_str(version).unwrap()
}

#[test]
fn linux_backend_stages_image_and_commits_trial() {
    let root = TempRoot::new();
    let mut images = FileSlotStorage::new(&root.0);
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();

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
        version: fixed_version("1.1.0"),
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
fn incomplete_replacement_preserves_previous_inactive_image() {
    let root = TempRoot::new();
    let mut images = FileSlotStorage::new(&root.0);
    let slot = images.begin_inactive_image(Slot::A, 8).unwrap();
    images.write_image_chunk(slot, 0, b"oldimage").unwrap();
    images.finish_inactive_image(slot).unwrap();

    let slot = images.begin_inactive_image(Slot::A, 10).unwrap();
    images.write_image_chunk(slot, 0, b"new").unwrap();
    assert!(images.finish_inactive_image(slot).is_err());
    images.abort_inactive_image(slot);

    assert_eq!(fs::read(images.image_path(Slot::B)).unwrap(), b"oldimage");
    assert!(!root.0.join("slots/b/sdv-image.tmp").exists());
}

#[test]
fn concurrent_update_attempts_are_rejected_until_abort_releases_lock() {
    let root = TempRoot::new();
    let mut first = FileSlotStorage::new(&root.0);
    let mut second = FileSlotStorage::new(&root.0);
    let slot = first.begin_inactive_image(Slot::A, 8).unwrap();

    assert!(matches!(
        second.begin_inactive_image(Slot::A, 8),
        Err(sdv_fota::linux::Error::UpdateInProgress)
    ));

    first.abort_inactive_image(slot);
    let second_slot = second.begin_inactive_image(Slot::A, 8).unwrap();
    second.abort_inactive_image(second_slot);
}

#[test]
fn corrupt_newest_boot_record_falls_back_to_previous_record() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
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
fn oversized_boot_record_is_ignored_in_favor_of_valid_copy() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();
    fs::write(root.0.join("bootctl.a"), vec![0u8; 4097]).unwrap();

    let state = boot.state().unwrap();

    assert_eq!(state.active, Slot::A);
    assert_eq!(state.active_version, "1.0.0");
    assert_eq!(state.trial.unwrap().slot, Slot::B);
}

#[test]
fn linux_launcher_commits_trial_after_health_command_succeeds() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
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
        version: fixed_version("1.1.0"),
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
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let slot_dir = root.0.join("slots/b");
    fs::create_dir_all(&slot_dir).unwrap();
    write_executable(&slot_dir.join("sdv-image.bin"), "#!/bin/sh\nexec sleep 2\n");
    let health = root.0.join("health-check");
    write_executable(&health, "#!/bin/sh\nexit 1\n");
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
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
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let slot_dir = root.0.join("slots/b");
    fs::create_dir_all(&slot_dir).unwrap();
    write_executable(&slot_dir.join("sdv-image.bin"), "#!/bin/sh\nexit 1\n");
    let health = root.0.join("health-check");
    write_executable(&health, "#!/bin/sh\nexit 0\n");
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
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

#[test]
fn install_cli_streams_signed_package_into_inactive_slot() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let payload = b"#!/bin/sh\nexec sleep 1\n";
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": "engine",
            "swc": "EngineSWC",
            "version": "1.1.0",
            "security_version": 2,
            "size_bytes": payload.len(),
            "sha256": hex(&sha256(payload))
        },
        "requires": {"signal_if_version": 1, "config_schema": [1]},
        "policy": {"allow_downgrade": false, "trial_ms": 5000}
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = ed25519_sign(&SIGNING_SEED, &manifest_bytes);
    let package = encode_package(KEY_ID, &signature, &manifest_bytes, payload).unwrap();
    let package_path = root.0.join("update.sdvf");
    fs::write(&package_path, package).unwrap();
    let public_key = ed25519_public_key(&SIGNING_SEED);

    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            root.0.to_str().unwrap(),
            package_path.to_str().unwrap(),
            KEY_ID,
            &hex(&public_key),
            "engine",
            "1",
            "1",
            "2",
        ])
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = boot.state().unwrap();
    let trial = state.trial.unwrap();
    assert_eq!(trial.slot, Slot::B);
    assert_eq!(trial.version, "1.1.0");
    assert_eq!(trial.trial_window_ms, 5_000);
    let reported = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args(["status", root.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(reported.status.success());
    assert!(String::from_utf8_lossy(&reported.stdout).contains("trial_slot: B"));
    let image_path = FileSlotStorage::new(&root.0).image_path(Slot::B);
    assert_eq!(fs::read(&image_path).unwrap(), payload);
    assert_ne!(
        fs::metadata(image_path).unwrap().permissions().mode() & 0o111,
        0
    );
}

#[cfg(feature = "host-signing")]
#[test]
fn signing_and_install_commands_complete_a_host_update() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let seed_path = root.0.join("signing.seed");
    fs::write(&seed_path, hex(&SIGNING_SEED)).unwrap();
    let image_path = root.0.join("candidate.bin");
    let payload = b"#!/bin/sh\nexec sleep 1\n";
    fs::write(&image_path, payload).unwrap();
    let package_path = root.0.join("candidate.sdvf");

    let signed = Command::new(env!("CARGO_BIN_EXE_sdv-fota-sign"))
        .args([
            seed_path.to_str().unwrap(),
            KEY_ID,
            "engine",
            "1.1.0",
            "2",
            "5000",
            "1",
            "1",
            image_path.to_str().unwrap(),
            package_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );

    let public_key = ed25519_public_key(&SIGNING_SEED);
    let installed = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            root.0.to_str().unwrap(),
            package_path.to_str().unwrap(),
            KEY_ID,
            &hex(&public_key),
            "engine",
            "1",
            "1",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let image_path = FileSlotStorage::new(&root.0).image_path(Slot::B);
    assert_eq!(fs::read(&image_path).unwrap(), payload);
    assert_eq!(boot.state().unwrap().trial.unwrap().version, "1.1.0");
    let health = root.0.join("health-check");
    write_executable(
        &health,
        "#!/bin/sh\ntest \"$SDV_TRIAL_SLOT\" = b && test -x \"$SDV_IMAGE_PATH\"\n",
    );

    let status = run_selected_image(&root.0, &health, &[]).unwrap();

    assert!(status.success());
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::B);
    assert_eq!(state.active_version, "1.1.0");
    assert!(state.trial.is_none());
}
