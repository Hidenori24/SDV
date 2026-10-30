#![cfg(target_os = "linux")]

use sdv_fota::linux::{
    run_active_target_image_as_with_groups_and_app_args, run_active_target_image_with_app_args,
    run_selected_image, run_selected_image_with_app_args, run_selected_target_image_with_app_args,
    run_trial_candidate_target_image_with_app_args, state_root_for_target, FileBootControl,
    FileSlotStorage, HttpsPackageSource,
};
use sdv_fota::{
    encode_package, BootControl, Ecu, ImageStorage, Slot, TrialImage, TrialOutcome, VersionString,
};
use sdv_sec::{ed25519_public_key, ed25519_sign, sha256};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

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

fn write_slot_image(root: &Path, slot: Slot, contents: &str) {
    let storage = FileSlotStorage::new(root);
    let path = storage.image_path(slot);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_executable(&path, contents);
    storage.record_image_integrity(slot).unwrap();
}

fn write_arg_recording_image(root: &Path, slot: Slot, log_path: &Path, stay_alive_for_trial: bool) {
    let log_path = log_path.to_string_lossy().replace('\'', "'\\''");
    let trial_behavior = if stay_alive_for_trial {
        "if [ \"${SDV_TRIAL_SLOT:-}\" = b ]; then exec sleep 8; fi\n"
    } else {
        ""
    };
    write_slot_image(
        root,
        slot,
        &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{log_path}'\n{trial_behavior}exit 0\n"),
    );
}

#[test]
fn https_ingress_rejects_plain_http_urls_and_embedded_credentials() {
    assert!(matches!(
        HttpsPackageSource::open("http://updates.example.test/update.sdvf"),
        Err(sdv_fota::linux::Error::InvalidHttpsUrl)
    ));
    assert!(matches!(
        HttpsPackageSource::open("https://user:password@updates.example.test/update.sdvf"),
        Err(sdv_fota::linux::Error::InvalidHttpsUrl)
    ));
}

#[test]
fn concurrent_trial_scheduling_keeps_one_consistent_boot_record() {
    let root = TempRoot::new();
    FileBootControl::new(&root.0)
        .initialize(Slot::A, "1.0.0", 1)
        .unwrap();
    let gate = Arc::new(Barrier::new(3));
    let workers: Vec<_> = ["1.1.0", "1.2.0"]
        .into_iter()
        .map(|version| {
            let root = root.0.clone();
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let mut boot = FileBootControl::new(root);
                gate.wait();
                boot.schedule_trial(TrialImage {
                    slot: Slot::B,
                    version: fixed_version(version),
                    security_version: 2,
                    boots_left: 3,
                    trial_window_ms: 5000,
                })
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(sdv_fota::linux::Error::TrialAlreadyPending)))
            .count(),
        1
    );
    let state = FileBootControl::new(&root.0).state().unwrap();
    assert!(matches!(
        state.trial.unwrap().version.as_str(),
        "1.1.0" | "1.2.0"
    ));
}

#[test]
fn exhausted_trial_boot_attempts_record_a_rollback_outcome() {
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

    assert!(boot.select_for_boot().unwrap().trial);
    let fallback = boot.select_for_boot().unwrap();
    assert_eq!(fallback.slot, Slot::A);
    assert!(!fallback.trial);

    let state = boot.state().unwrap();
    let result = state.last_trial.unwrap();
    assert_eq!(result.slot, Slot::B);
    assert_eq!(result.version, "1.1.0");
    assert_eq!(result.outcome, TrialOutcome::RolledBack);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(feature = "host-signing")]
fn write_seed_file(path: &Path, seed: &[u8; 32], mode: u32) {
    fs::write(path, hex(seed)).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_trust_store(root: &TempRoot) -> PathBuf {
    write_trust_store_with_seed(root, KEY_ID, &SIGNING_SEED)
}

fn write_trust_store_with_seed(root: &TempRoot, key_id: &str, seed: &[u8; 32]) -> PathBuf {
    let path = root.0.join("trusted-keys");
    let public_key = ed25519_public_key(seed);
    fs::write(&path, format!("{key_id} {}\n", hex(&public_key))).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    path
}

fn test_ecu_state_root(base: &Path, target: &str) -> PathBuf {
    match target {
        "vehicle" => base.to_path_buf(),
        "engine" => base.join("ecus").join("engine"),
        "brake" => base.join("ecus").join("brake"),
        "steering" => base.join("ecus").join("steering"),
        _ => panic!("unexpected test ECU target"),
    }
}

fn signed_test_package(
    target: &str,
    version: &str,
    security_version: u64,
    payload: &[u8],
) -> Vec<u8> {
    let (target_ecu, swc) = match target {
        "engine" => ("engine", "EngineSWC"),
        "brake" => ("brake", "BrakeSWC"),
        "steering" => ("steering", "SteeringSWC"),
        "vehicle" => ("vehicle", "VehicleControlApp"),
        _ => panic!("unexpected test ECU target"),
    };
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": target_ecu,
            "swc": swc,
            "version": version,
            "security_version": security_version,
            "size_bytes": payload.len(),
            "sha256": hex(&sha256(payload))
        },
        "requires": {"signal_if_version": 1, "config_schema": [1]},
        "policy": {"allow_downgrade": false, "trial_ms": 5000}
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = ed25519_sign(&SIGNING_SEED, &manifest_bytes);
    encode_package(KEY_ID, &signature, &manifest_bytes, payload).unwrap()
}

fn initialize_ecu_from_cli(
    base_root: &Path,
    target: &str,
    active_version: &str,
    security_version: u64,
) {
    let target_root = test_ecu_state_root(base_root, target);
    let active_image = FileSlotStorage::new(&target_root).image_path(Slot::A);
    fs::create_dir_all(active_image.parent().unwrap()).unwrap();
    write_executable(&active_image, "#!/bin/sh\nexit 0\n");

    let initialized = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "init",
            "--state-root",
            base_root.to_str().unwrap(),
            "--target",
            target,
            "--active-slot",
            "a",
            "--active-version",
            active_version,
            "--security-version",
        ])
        .arg(security_version.to_string())
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
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
    assert_eq!(state.last_trial.unwrap().outcome, TrialOutcome::Committed);
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
    let candidate_trial_slot = root.0.join("candidate-trial-slot");
    write_slot_image(
        &root.0,
        Slot::B,
        &format!(
            "#!/bin/sh\nprintf '%s' \"$SDV_TRIAL_SLOT\" > {}\nexec sleep 2\n",
            candidate_trial_slot.display()
        ),
    );
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
    assert_eq!(fs::read_to_string(candidate_trial_slot).unwrap(), "b");
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::B);
    assert_eq!(state.active_version, "1.1.0");
    assert!(state.trial.is_none());
}

#[test]
fn launcher_usage_errors_keep_the_legacy_exit_code() {
    let missing_arguments = Command::new(env!("CARGO_BIN_EXE_sdv-fota-launcher"))
        .output()
        .unwrap();
    assert_eq!(missing_arguments.status.code(), Some(2));

    let invalid_target = Command::new(env!("CARGO_BIN_EXE_sdv-fota-launcher"))
        .args([
            "--state-root",
            "/var/lib/sdv-fota",
            "--target",
            "ecu/../../vehicle",
            "--health-program",
            "/usr/bin/health",
        ])
        .output()
        .unwrap();
    assert_eq!(invalid_target.status.code(), Some(2));
}

#[test]
fn launcher_passes_each_fixed_app_argument_without_shell_interpretation_on_active_and_trial() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();

    let sentinel = root.0.join("shell-command-ran");
    let arguments = [
        OsString::from("one argument with spaces"),
        OsString::from(format!("; touch {}", sentinel.display())),
        OsString::from(format!("$(touch {})", sentinel.display())),
        OsString::from(format!("`touch {}`", sentinel.display())),
    ];
    let active_log = root.0.join("active-arguments.log");
    write_arg_recording_image(&root.0, Slot::A, &active_log, false);

    let active =
        run_selected_image_with_app_args(&root.0, Path::new("/bin/true"), &[], &arguments).unwrap();
    assert!(active.success());
    assert_eq!(
        fs::read_to_string(&active_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        arguments
            .iter()
            .map(|argument| argument.to_str())
            .collect::<Option<Vec<_>>>()
            .unwrap()
    );
    assert!(!sentinel.exists());

    let trial_log = root.0.join("trial-arguments.log");
    write_arg_recording_image(&root.0, Slot::B, &trial_log, true);
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();

    let trial =
        run_selected_image_with_app_args(&root.0, Path::new("/bin/true"), &[], &arguments).unwrap();
    assert!(trial.success());
    assert_eq!(
        fs::read_to_string(&trial_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        arguments
            .iter()
            .map(|argument| argument.to_str())
            .collect::<Option<Vec<_>>>()
            .unwrap()
    );
    assert!(!sentinel.exists());
    assert_eq!(boot.state().unwrap().active, Slot::B);
}

#[test]
fn selecting_one_target_trial_does_not_change_another_ecus_ab_state() {
    let root = TempRoot::new();
    let engine_root = state_root_for_target(&root.0, Ecu::Engine);
    let brake_root = state_root_for_target(&root.0, Ecu::Brake);
    assert_eq!(engine_root, root.0.join("ecus/engine"));
    assert_eq!(brake_root, root.0.join("ecus/brake"));

    let mut engine_boot = FileBootControl::new(&engine_root);
    engine_boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(&engine_root, Slot::A, "#!/bin/sh\nexit 0\n");
    write_slot_image(&engine_root, Slot::B, "#!/bin/sh\nexec sleep 8\n");
    engine_boot
        .schedule_trial(TrialImage {
            slot: Slot::B,
            version: fixed_version("1.1.0"),
            security_version: 2,
            boots_left: 2,
            trial_window_ms: 5_000,
        })
        .unwrap();

    let mut brake_boot = FileBootControl::new(&brake_root);
    brake_boot.initialize(Slot::A, "2.0.0", 3).unwrap();
    brake_boot
        .schedule_trial(TrialImage {
            slot: Slot::B,
            version: fixed_version("2.1.0"),
            security_version: 4,
            boots_left: 2,
            trial_window_ms: 5_000,
        })
        .unwrap();
    let brake_before = brake_boot.state().unwrap();

    let status = run_selected_image_with_app_args(
        &engine_root,
        Path::new("/bin/true"),
        &[],
        &["--ecu".into(), "engine".into()],
    )
    .unwrap();
    assert!(status.success());

    let engine_state = engine_boot.state().unwrap();
    assert_eq!(engine_state.active, Slot::B);
    assert_eq!(engine_state.active_version, "1.1.0");
    assert!(engine_state.trial.is_none());
    assert_eq!(brake_boot.state().unwrap(), brake_before);
}

#[test]
fn ecu_target_launcher_rejects_missing_duplicate_or_mismatched_ecu_selector() {
    let root = TempRoot::new();
    let engine_root = state_root_for_target(&root.0, Ecu::Engine);
    fs::create_dir_all(&root.0.join("ecus")).unwrap();
    let mut boot = FileBootControl::new(&engine_root);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();

    for args in [
        vec!["--worker-socket".into(), "/run/worker.sock".into()],
        vec!["--ecu".into(), "brake".into()],
        vec![
            "--ecu".into(),
            "engine".into(),
            "--ecu".into(),
            "engine".into(),
        ],
    ] {
        assert!(matches!(
            run_active_target_image_as_with_groups_and_app_args(
                &root.0,
                Ecu::Engine,
                1000,
                1000,
                &[],
                &args,
            ),
            Err(sdv_fota::linux::Error::InvalidTargetArguments)
        ));
        assert!(matches!(
            run_active_target_image_with_app_args(&root.0, Ecu::Engine, &args,),
            Err(sdv_fota::linux::Error::InvalidTargetArguments)
        ));
        assert_eq!(boot.state().unwrap().trial.unwrap().boots_left, 2);
    }
}

#[test]
fn ecu_target_launcher_rejects_an_ecus_symlink_ancestor() {
    let root = TempRoot::new();
    let redirected = root.0.join("redirected-ecus");
    fs::create_dir_all(&redirected).unwrap();
    std::os::unix::fs::symlink(&redirected, root.0.join("ecus")).unwrap();

    assert!(matches!(
        run_active_target_image_as_with_groups_and_app_args(
            &root.0,
            Ecu::Engine,
            1000,
            1000,
            &[],
            &["--ecu".into(), "engine".into()],
        ),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
    assert!(matches!(
        run_active_target_image_with_app_args(
            &root.0,
            Ecu::Engine,
            &["--ecu".into(), "engine".into()],
        ),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
}

#[test]
fn selected_target_apis_refuse_ecu_trial_launches() {
    let root = TempRoot::new();
    let args = ["--ecu".into(), "engine".into()];
    let result = run_selected_target_image_with_app_args(
        &root.0,
        Ecu::Engine,
        Path::new("/bin/true"),
        &[],
        &args,
    );
    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::InvalidTargetLaunchMode)
    ));
    assert!(matches!(
        sdv_fota::linux::run_selected_target_image_as_with_groups_and_app_args(
            &root.0,
            Ecu::Engine,
            Path::new("/bin/true"),
            &[],
            1000,
            1000,
            &[],
            &args,
        ),
        Err(sdv_fota::linux::Error::InvalidTargetLaunchMode)
    ));
}

#[test]
fn active_ecu_launch_runs_active_image_without_changing_pending_trial_state() {
    let root = TempRoot::new();
    let engine_root = state_root_for_target(&root.0, Ecu::Engine);
    let active_log = root.0.join("active-ecu-argv.txt");
    let candidate_marker = root.0.join("candidate-was-run");

    let mut boot = FileBootControl::new(&engine_root);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(
        &engine_root,
        Slot::A,
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
if [ -n "${{SDV_TRIAL_SLOT:-}}" ] || [ -n "${{SDV_IMAGE_PATH:-}}" ]; then exit 42; fi
"#,
            active_log.display(),
        ),
    );
    write_slot_image(
        &engine_root,
        Slot::B,
        &format!("#!/bin/sh\ntouch '{}'\n", candidate_marker.display()),
    );
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();
    let before = boot.state().unwrap();
    let boot_a_before = fs::read(engine_root.join("bootctl.a")).unwrap();
    let boot_b_before = fs::read(engine_root.join("bootctl.b")).unwrap();
    let app_args = [
        OsString::from("--ecu"),
        OsString::from("engine"),
        OsString::from("one worker arg"),
    ];

    let status = run_active_target_image_with_app_args(&root.0, Ecu::Engine, &app_args).unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(active_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["--ecu", "engine", "one worker arg"]
    );
    assert!(!candidate_marker.exists());
    assert_eq!(boot.state().unwrap(), before);
    assert_eq!(
        fs::read(engine_root.join("bootctl.a")).unwrap(),
        boot_a_before
    );
    assert_eq!(
        fs::read(engine_root.join("bootctl.b")).unwrap(),
        boot_b_before
    );
}

#[test]
fn ecu_trial_candidate_runs_pending_image_without_changing_any_boot_metadata() {
    let root = TempRoot::new();
    fs::create_dir_all(root.0.join("ecus")).unwrap();
    let engine_root = state_root_for_target(&root.0, Ecu::Engine);
    let candidate_log = root.0.join("candidate-ecu-argv.txt");
    let candidate_env = root.0.join("candidate-ecu-env.txt");
    let mut boot = FileBootControl::new(&engine_root);
    boot.initialize(Slot::A, "1.0.0", 7).unwrap();
    write_slot_image(&engine_root, Slot::A, "#!/bin/sh\nexit 0\n");
    write_slot_image(
        &engine_root,
        Slot::B,
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
printf '%s|%s\n' "${{SDV_TRIAL_SLOT:-}}" "${{SDV_TRIAL_CANDIDATE_SLOT:-}}" > '{}'
"#,
            candidate_log.display(),
            candidate_env.display(),
        ),
    );
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 8,
        boots_left: 3,
        trial_window_ms: 5_000,
    })
    .unwrap();
    let state_before = boot.state().unwrap();
    let record_a_before = fs::read(engine_root.join("bootctl.a")).unwrap();
    let record_b_before = fs::read(engine_root.join("bootctl.b")).unwrap();
    let active_digest_before = fs::read(
        FileSlotStorage::new(&engine_root)
            .image_path(Slot::A)
            .with_file_name("sdv-image.sha256"),
    )
    .unwrap();
    let candidate_digest_before = fs::read(
        FileSlotStorage::new(&engine_root)
            .image_path(Slot::B)
            .with_file_name("sdv-image.sha256"),
    )
    .unwrap();
    let app_args = [
        OsString::from("--ecu"),
        OsString::from("engine"),
        OsString::from("--worker-socket"),
        OsString::from("/run/sdv-picarx/candidates/engine.sock"),
        OsString::from("one argument with spaces"),
    ];

    let status =
        run_trial_candidate_target_image_with_app_args(&root.0, Ecu::Engine, &app_args).unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(candidate_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "--ecu",
            "engine",
            "--worker-socket",
            "/run/sdv-picarx/candidates/engine.sock",
            "one argument with spaces",
        ]
    );
    assert_eq!(fs::read_to_string(candidate_env).unwrap().trim(), "|b");
    assert_eq!(boot.state().unwrap(), state_before);
    assert_eq!(
        fs::read(engine_root.join("bootctl.a")).unwrap(),
        record_a_before
    );
    assert_eq!(
        fs::read(engine_root.join("bootctl.b")).unwrap(),
        record_b_before
    );
    assert_eq!(
        fs::read(
            FileSlotStorage::new(&engine_root)
                .image_path(Slot::A)
                .with_file_name("sdv-image.sha256"),
        )
        .unwrap(),
        active_digest_before
    );
    assert_eq!(
        fs::read(
            FileSlotStorage::new(&engine_root)
                .image_path(Slot::B)
                .with_file_name("sdv-image.sha256"),
        )
        .unwrap(),
        candidate_digest_before
    );
}

#[test]
fn ecu_trial_candidate_releases_image_update_lock_after_exec_starts() {
    let root = TempRoot::new();
    fs::create_dir_all(root.0.join("ecus")).unwrap();
    let engine_root = state_root_for_target(&root.0, Ecu::Engine);
    let started_marker = root.0.join("candidate-started");
    let mut boot = FileBootControl::new(&engine_root);
    boot.initialize(Slot::A, "1.0.0", 7).unwrap();
    write_slot_image(&engine_root, Slot::A, "#!/bin/sh\nexit 0\n");
    write_slot_image(
        &engine_root,
        Slot::B,
        &format!("#!/bin/sh\ntouch '{}'\nsleep 1\n", started_marker.display()),
    );
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 8,
        boots_left: 3,
        trial_window_ms: 5_000,
    })
    .unwrap();
    let root_path = root.0.clone();
    let candidate = std::thread::spawn(move || {
        run_trial_candidate_target_image_with_app_args(
            root_path,
            Ecu::Engine,
            &[OsString::from("--ecu"), OsString::from("engine")],
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !started_marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(started_marker.exists(), "candidate image did not start");

    let mut storage = FileSlotStorage::new(&engine_root);
    assert!(storage.begin_inactive_image(Slot::A, 1).is_ok());
    drop(storage);
    assert!(candidate.join().unwrap().unwrap().success());
}

#[test]
fn linux_launcher_rolls_back_if_image_exits_during_health_stability_window() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(&root.0, Slot::B, "#!/bin/sh\nsleep 0.1\nexit 0\n");
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
fn linux_launcher_runs_trial_image_and_health_check_as_configured_user() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let run_as = fs::metadata("/proc/self").unwrap();
    let uid_record = root.0.join("candidate-uid");
    write_slot_image(
        &root.0,
        Slot::B,
        &format!(
            "#!/bin/sh\nid -u > {}\nexec sleep 2\n",
            uid_record.display()
        ),
    );
    let health = root.0.join("health-check");
    write_executable(
        &health,
        &format!("#!/bin/sh\ntest \"$(id -u)\" = {}\n", run_as.uid()),
    );
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 2_000,
    })
    .unwrap();

    let status =
        sdv_fota::linux::run_selected_image_as(&root.0, &health, &[], run_as.uid(), run_as.gid())
            .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(uid_record).unwrap().trim(),
        run_as.uid().to_string()
    );
    assert_eq!(boot.state().unwrap().active, Slot::B);
}

#[test]
fn linux_launcher_refuses_root_as_application_identity() {
    let root = TempRoot::new();
    let result = sdv_fota::linux::run_selected_image_as(&root.0, Path::new("/bin/true"), &[], 0, 0);
    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::InvalidRunIdentity)
    ));
    assert!(!root.0.join("bootctl.a").exists());
}

#[test]
fn linux_launcher_refuses_root_supplementary_group() {
    let root = TempRoot::new();
    let result = sdv_fota::linux::run_selected_image_as_with_groups(
        &root.0,
        Path::new("/bin/true"),
        &[],
        1000,
        1000,
        &[1001, 0],
    );
    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::InvalidRunIdentity)
    ));
    assert!(!root.0.join("bootctl.a").exists());
}

#[test]
fn linux_launcher_rolls_back_trial_when_health_window_expires() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(&root.0, Slot::B, "#!/bin/sh\nexec sleep 2\n");
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
    assert_eq!(state.last_trial.unwrap().outcome, TrialOutcome::RolledBack);
}

#[test]
fn linux_launcher_rolls_back_if_trial_image_exits_before_health_check() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(&root.0, Slot::B, "#!/bin/sh\nexit 1\n");
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
fn init_cli_accepts_named_provisioning_options() {
    let root = TempRoot::new();
    let image_path = FileSlotStorage::new(&root.0).image_path(Slot::B);
    fs::create_dir_all(image_path.parent().unwrap()).unwrap();
    write_executable(&image_path, "#!/bin/sh\nexit 0\n");

    let initialized = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "init",
            "--security-version",
            "7",
            "--active-version",
            "2.3.0",
            "--state-root",
            root.0.to_str().unwrap(),
            "--active-slot",
            "b",
        ])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    let status = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args(["status", "--state-root", root.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(status.status.success());
    let report = String::from_utf8_lossy(&status.stdout);
    assert!(report.contains("active_slot: B"));
    assert!(report.contains("active_version: 2.3.0"));
    assert!(report.contains("highest_security_version: 7"));
    assert!(report.contains("active_image_integrity: verified"));
    assert!(report.contains("last_trial_outcome: none"));
    FileSlotStorage::new(&root.0)
        .verify_executable_image(Slot::B)
        .unwrap();
}

#[test]
fn rollback_trial_cli_clears_pending_candidate_and_rejects_empty_rollback() {
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

    let rollback = || {
        Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args(["rollback-trial", "--state-root", root.0.to_str().unwrap()])
            .output()
            .unwrap()
    };
    let result = rollback();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert_eq!(state.active_version, "1.0.0");
    assert!(state.trial.is_none());
    assert_eq!(state.last_trial.unwrap().outcome, TrialOutcome::RolledBack);

    let status = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args(["status", "--state-root", root.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(status.status.success());
    let report = String::from_utf8_lossy(&status.stdout);
    assert!(report.contains("last_trial_outcome: rolled_back"));
    assert!(report.contains("last_trial_version: 1.1.0"));

    let repeated = rollback();
    assert!(!repeated.status.success());
    assert!(String::from_utf8_lossy(&repeated.stderr).contains("no pending trial"));
}

#[test]
fn verify_images_cli_returns_failure_when_a_trial_digest_mismatches() {
    let root = TempRoot::new();
    let storage = FileSlotStorage::new(&root.0);
    write_slot_image(&root.0, Slot::A, "#!/bin/sh\nexit 0\n");
    write_slot_image(&root.0, Slot::B, "#!/bin/sh\nexit 0\n");
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize_with_image(&storage, Slot::A, "1.0.0", 1)
        .unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();

    let verify = || {
        Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args(["verify-images", "--state-root", root.0.to_str().unwrap()])
            .output()
            .unwrap()
    };
    let healthy = verify();
    assert!(
        healthy.status.success(),
        "{}",
        String::from_utf8_lossy(&healthy.stderr)
    );
    let healthy_report = String::from_utf8_lossy(&healthy.stdout);
    assert!(healthy_report.contains("active_image_integrity: verified"));
    assert!(healthy_report.contains("trial_image_integrity: verified"));

    fs::write(storage.image_path(Slot::B), "#!/bin/sh\nexit 1\n").unwrap();
    let corrupt = verify();
    assert!(!corrupt.status.success());
    let corrupt_report = String::from_utf8_lossy(&corrupt.stdout);
    assert!(corrupt_report.contains("active_image_integrity: verified"));
    assert!(corrupt_report.contains("trial_image_integrity: mismatch_or_unbootable"));
}

#[test]
fn record_active_integrity_only_updates_the_active_slot_without_a_trial() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    let image = FileSlotStorage::new(&root.0).image_path(Slot::A);
    fs::create_dir_all(image.parent().unwrap()).unwrap();
    write_executable(&image, "#!/bin/sh\nexit 0\n");
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let record_integrity = |slot: &str| {
        Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args([
                "record-active-integrity",
                "--state-root",
                root.0.to_str().unwrap(),
                "--slot",
                slot,
            ])
            .output()
            .unwrap()
    };

    assert!(!record_integrity("b").status.success());
    assert!(!root.0.join("slots/a/sdv-image.sha256").exists());

    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();
    assert!(!record_integrity("a").status.success());
    assert!(!root.0.join("slots/a/sdv-image.sha256").exists());

    boot.rollback_trial().unwrap();
    let result = record_integrity("a");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    FileSlotStorage::new(&root.0)
        .verify_executable_image(Slot::A)
        .unwrap();
}

#[test]
fn status_reports_missing_and_mismatched_active_image_digests() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    let image = FileSlotStorage::new(&root.0).image_path(Slot::A);
    fs::create_dir_all(image.parent().unwrap()).unwrap();
    write_executable(&image, "#!/bin/sh\nexit 0\n");
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let status = || {
        Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args(["status", "--state-root", root.0.to_str().unwrap()])
            .output()
            .unwrap()
    };

    let missing = status();
    assert!(missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stdout).contains("active_image_integrity: missing"));

    FileSlotStorage::new(&root.0)
        .record_image_integrity(Slot::A)
        .unwrap();
    fs::write(&image, "#!/bin/sh\nexit 1\n").unwrap();
    let mismatched = status();
    assert!(mismatched.status.success());
    assert!(String::from_utf8_lossy(&mismatched.stdout)
        .contains("active_image_integrity: mismatch_or_unbootable"));
}

#[test]
fn init_cli_rejects_missing_or_nonexecutable_active_image() {
    let root = TempRoot::new();
    let image_path = FileSlotStorage::new(&root.0).image_path(Slot::A);
    let initialize = || {
        Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args([
                "init",
                "--state-root",
                root.0.to_str().unwrap(),
                "--active-slot",
                "a",
                "--active-version",
                "1.0.0",
                "--security-version",
                "1",
            ])
            .output()
            .unwrap()
    };

    assert!(!initialize().status.success());
    assert!(!root.0.join("bootctl.a").exists());
    fs::create_dir_all(image_path.parent().unwrap()).unwrap();
    fs::write(&image_path, "not executable").unwrap();
    assert!(!initialize().status.success());
    assert!(!root.0.join("bootctl.a").exists());
    fs::write(&image_path, "").unwrap();
    let mut permissions = fs::metadata(&image_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&image_path, permissions).unwrap();
    assert!(!initialize().status.success());
    assert!(!root.0.join("bootctl.a").exists());
}

#[test]
fn boot_metadata_refuses_group_or_other_writable_state_root() {
    let root = TempRoot::new();
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o777)).unwrap();

    let result = FileBootControl::new(&root.0).initialize(Slot::A, "1.0.0", 1);

    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
    assert!(!root.0.join("bootctl.a").exists());
}

#[test]
fn boot_metadata_rejects_a_symlinked_state_root() {
    let root = TempRoot::new();
    let target = root.0.join("target");
    let state_root = root.0.join("state-root");
    fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &state_root).unwrap();

    assert!(matches!(
        FileBootControl::new(state_root).state(),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
}

#[test]
fn boot_metadata_lock_does_not_follow_a_symlink() {
    let root = TempRoot::new();
    let outside = root.0.join("outside");
    fs::write(&outside, b"preserve").unwrap();
    std::os::unix::fs::symlink(&outside, root.0.join(".fota-bootctl.lock")).unwrap();

    let result = FileBootControl::new(&root.0).initialize(Slot::A, "1.0.0", 1);

    assert!(result.is_err());
    assert_eq!(fs::read(outside).unwrap(), b"preserve");
    assert!(!root.0.join("bootctl.a").exists());
}

#[test]
fn boot_image_validation_rejects_group_writable_image_and_slot_directory() {
    let root = TempRoot::new();
    let storage = FileSlotStorage::new(&root.0);
    let image = storage.image_path(Slot::A);
    fs::create_dir_all(image.parent().unwrap()).unwrap();
    write_executable(&image, "#!/bin/sh\nexit 0\n");
    fs::set_permissions(&image, fs::Permissions::from_mode(0o775)).unwrap();

    assert!(matches!(
        storage.validate_executable_image(Slot::A),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));

    fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(image.parent().unwrap(), fs::Permissions::from_mode(0o775)).unwrap();
    assert!(matches!(
        storage.validate_executable_image(Slot::A),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
}

#[test]
fn boot_image_validation_rejects_a_symlinked_slot_directory() {
    let root = TempRoot::new();
    let storage = FileSlotStorage::new(&root.0);
    let slots = root.0.join("slots");
    let target = root.0.join("external-slot");
    fs::create_dir(&slots).unwrap();
    fs::create_dir(&target).unwrap();
    write_executable(&target.join("sdv-image.bin"), "#!/bin/sh\nexit 0\n");
    std::os::unix::fs::symlink(target, slots.join("a")).unwrap();

    assert!(matches!(
        storage.validate_executable_image(Slot::A),
        Err(sdv_fota::linux::Error::UnsafeStatePath)
    ));
}

#[test]
fn executable_image_verification_does_not_follow_digest_symlinks() {
    let root = TempRoot::new();
    let storage = FileSlotStorage::new(&root.0);
    write_slot_image(&root.0, Slot::A, "#!/bin/sh\nexit 0\n");
    let record = storage
        .image_path(Slot::A)
        .with_file_name("sdv-image.sha256");
    let external_record = root.0.join("external-digest");
    fs::copy(&record, &external_record).unwrap();
    fs::remove_file(&record).unwrap();
    std::os::unix::fs::symlink(&external_record, &record).unwrap();

    assert!(storage.verify_executable_image(Slot::A).is_err());
}

#[test]
fn launcher_rolls_back_trial_with_nonexecutable_candidate_image() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let candidate = FileSlotStorage::new(&root.0).image_path(Slot::B);
    fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    fs::write(&candidate, "not executable").unwrap();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: fixed_version("1.1.0"),
        security_version: 2,
        boots_left: 2,
        trial_window_ms: 5_000,
    })
    .unwrap();

    assert!(matches!(
        run_selected_image(&root.0, Path::new("/bin/true"), &[]),
        Err(sdv_fota::linux::Error::InvalidBootImage)
    ));
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert!(state.trial.is_none());
}

#[test]
fn launcher_rolls_back_trial_when_image_digest_mismatches() {
    let root = TempRoot::new();
    let mut boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    write_slot_image(&root.0, Slot::B, "#!/bin/sh\nexec sleep 2\n");
    fs::write(
        FileSlotStorage::new(&root.0).image_path(Slot::B),
        "#!/bin/sh\nexit 1\n",
    )
    .unwrap();
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
        Err(sdv_fota::linux::Error::InvalidBootImage)
    ));
    let state = boot.state().unwrap();
    assert_eq!(state.active, Slot::A);
    assert!(state.trial.is_none());
}

#[test]
fn launcher_does_not_execute_active_image_after_digest_mismatch() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    let started = root.0.join("started");
    let image = format!("#!/bin/sh\ntouch {}\n", started.display());
    write_slot_image(&root.0, Slot::A, &image);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    fs::write(
        FileSlotStorage::new(&root.0).image_path(Slot::A),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();

    let result = run_selected_image(&root.0, Path::new("/bin/true"), &[]);

    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::InvalidBootImage)
    ));
    assert!(!started.exists());
}

#[test]
fn linux_cli_keeps_per_ecu_slots_versions_and_trials_independent() {
    let root = TempRoot::new();
    initialize_ecu_from_cli(&root.0, "engine", "1.0.0", 10);
    initialize_ecu_from_cli(&root.0, "brake", "2.0.0", 20);
    initialize_ecu_from_cli(&root.0, "steering", "3.0.0", 30);
    initialize_ecu_from_cli(&root.0, "vehicle", "4.0.0", 40);
    let trust_store = write_trust_store(&root);

    let engine_payload = b"#!/bin/sh\n# engine image\n";
    let engine_package_path = root.0.join("engine.sdvf");
    fs::write(
        &engine_package_path,
        signed_test_package("engine", "1.1.0", 11, engine_payload),
    )
    .unwrap();
    let brake_payload = b"#!/bin/sh\n# brake image\n";
    let brake_package_path = root.0.join("brake.sdvf");
    fs::write(
        &brake_package_path,
        signed_test_package("brake", "2.1.0", 21, brake_payload),
    )
    .unwrap();
    let wrong_target_package_path = root.0.join("wrong-target.sdvf");
    fs::write(
        &wrong_target_package_path,
        signed_test_package("engine", "3.1.0", 31, b"#!/bin/sh\n# wrong target\n"),
    )
    .unwrap();

    for (target, package_path) in [
        ("engine", &engine_package_path),
        ("brake", &brake_package_path),
    ] {
        let installed = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args([
                "install",
                "--state-root",
                root.0.to_str().unwrap(),
                "--package",
                package_path.to_str().unwrap(),
                "--target",
                target,
                "--config-schema",
                "1",
                "--signal-if-version",
                "1",
            ])
            .env("SDV_FOTA_TEST_TRUST_STORE", &trust_store)
            .output()
            .unwrap();
        assert!(
            installed.status.success(),
            "{}",
            String::from_utf8_lossy(&installed.stderr)
        );
    }

    let engine_root = test_ecu_state_root(&root.0, "engine");
    let brake_root = test_ecu_state_root(&root.0, "brake");
    let steering_root = test_ecu_state_root(&root.0, "steering");
    let engine_state = FileBootControl::new(&engine_root).state().unwrap();
    let brake_state = FileBootControl::new(&brake_root).state().unwrap();
    let vehicle_state = FileBootControl::new(&root.0).state().unwrap();
    assert_eq!(engine_state.active_version, "1.0.0");
    assert_eq!(engine_state.highest_security_version, 10);
    assert_eq!(engine_state.trial.unwrap().version, "1.1.0");
    assert_eq!(engine_state.trial.unwrap().security_version, 11);
    assert_eq!(brake_state.active_version, "2.0.0");
    assert_eq!(brake_state.highest_security_version, 20);
    assert_eq!(brake_state.trial.unwrap().version, "2.1.0");
    assert_eq!(brake_state.trial.unwrap().security_version, 21);
    assert_eq!(vehicle_state.active_version, "4.0.0");
    assert_eq!(vehicle_state.highest_security_version, 40);
    assert!(vehicle_state.trial.is_none());
    assert_eq!(
        fs::read(FileSlotStorage::new(&engine_root).image_path(Slot::B)).unwrap(),
        engine_payload
    );
    assert_eq!(
        fs::read(FileSlotStorage::new(&brake_root).image_path(Slot::B)).unwrap(),
        brake_payload
    );

    let mismatch = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            "--state-root",
            root.0.to_str().unwrap(),
            "--package",
            wrong_target_package_path.to_str().unwrap(),
            "--target",
            "steering",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
        ])
        .env("SDV_FOTA_TEST_TRUST_STORE", &trust_store)
        .output()
        .unwrap();
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("WrongTarget"));
    assert!(FileBootControl::new(&steering_root)
        .state()
        .unwrap()
        .trial
        .is_none());
    assert!(!FileSlotStorage::new(&steering_root)
        .image_path(Slot::B)
        .exists());

    let integrity = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "record-active-integrity",
            "--state-root",
            root.0.to_str().unwrap(),
            "--target",
            "steering",
            "--slot",
            "a",
        ])
        .output()
        .unwrap();
    assert!(
        integrity.status.success(),
        "{}",
        String::from_utf8_lossy(&integrity.stderr)
    );

    let rollback = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "rollback-trial",
            "--state-root",
            root.0.to_str().unwrap(),
            "--target",
            "engine",
        ])
        .output()
        .unwrap();
    assert!(rollback.status.success());
    assert!(FileBootControl::new(&engine_root)
        .state()
        .unwrap()
        .trial
        .is_none());
    assert!(FileBootControl::new(&brake_root)
        .state()
        .unwrap()
        .trial
        .is_some());

    for target in ["engine", "brake"] {
        let verified = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
            .args([
                "verify-images",
                "--state-root",
                root.0.to_str().unwrap(),
                "--target",
                target,
            ])
            .output()
            .unwrap();
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
    }

    let vehicle_status = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args(["status", "--state-root", root.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(vehicle_status.status.success());
    assert!(String::from_utf8_lossy(&vehicle_status.stdout).contains("active_version: 4.0.0"));
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
            "target_ecu": "vehicle",
            "swc": "VehicleControlApp",
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
    let trust_store = write_trust_store(&root);

    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            "--state-root",
            root.0.to_str().unwrap(),
            "--package",
            package_path.to_str().unwrap(),
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--trial-boots",
            "2",
        ])
        .env("SDV_FOTA_TEST_TRUST_STORE", trust_store)
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
        .args(["status", "--state-root", root.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(reported.status.success());
    let report = String::from_utf8_lossy(&reported.stdout);
    assert!(report.contains("trial_slot: B"));
    assert!(report.contains("trial_image_integrity: verified"));
    let image_path = FileSlotStorage::new(&root.0).image_path(Slot::B);
    assert_eq!(fs::read(&image_path).unwrap(), payload);
    assert_eq!(
        fs::read_to_string(image_path.with_file_name("sdv-image.sha256")).unwrap(),
        hex(&sha256(payload))
    );
    assert_ne!(
        fs::metadata(image_path).unwrap().permissions().mode() & 0o111,
        0
    );
}

#[test]
fn install_cli_rejects_package_signed_by_untrusted_key() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let payload = b"candidate image";
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": "vehicle",
            "swc": "VehicleControlApp",
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
    let package_path = root.0.join("untrusted.sdvf");
    fs::write(&package_path, package).unwrap();
    let trust_store = write_trust_store_with_seed(&root, KEY_ID, &[9; 32]);

    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            "--state-root",
            root.0.to_str().unwrap(),
            "--package",
            package_path.to_str().unwrap(),
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
        ])
        .env("SDV_FOTA_TEST_TRUST_STORE", trust_store)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(boot.state().unwrap().trial.is_none());
    assert!(!FileSlotStorage::new(&root.0).image_path(Slot::B).exists());
}

#[test]
fn install_url_streams_https_package_through_curl_ingress() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let payload = b"#! /bin/sh\nexit 0\n";
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": "vehicle",
            "swc": "VehicleControlApp",
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
    let package_path = root.0.join("network-response.sdvf");
    fs::write(&package_path, package).unwrap();
    let trust_store = write_trust_store(&root);

    let certificate = root.0.join("client.crt");
    fs::write(&certificate, b"test certificate").unwrap();
    fs::set_permissions(&certificate, fs::Permissions::from_mode(0o644)).unwrap();
    let private_key = root.0.join("client.key");
    fs::write(&private_key, b"test private key").unwrap();
    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o600)).unwrap();
    let curl_arguments = root.0.join("curl-arguments");

    let fake_bin = root.0.join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    write_executable(
        fake_bin.join("curl").as_path(),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SDV_FOTA_TEST_CURL_ARGS\"\ncat \"$SDV_FOTA_TEST_PACKAGE\"\n",
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var_os("PATH")
            .unwrap_or_default()
            .to_string_lossy()
    );
    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install-url",
            "--state-root",
            root.0.to_str().unwrap(),
            "--url",
            "https://updates.example.test/vehicle.sdvf",
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--trial-boots",
            "2",
            "--client-cert",
            certificate.to_str().unwrap(),
            "--client-key",
            private_key.to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("SDV_FOTA_TEST_PACKAGE", &package_path)
        .env("SDV_FOTA_TEST_CURL_ARGS", &curl_arguments)
        .env("SDV_FOTA_TEST_TRUST_STORE", trust_store)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let captured_args = fs::read_to_string(curl_arguments).unwrap();
    assert!(captured_args.contains(&format!(
        "--cert\n{}\n--key\n{}\n",
        certificate.display(),
        private_key.display()
    )));
    assert!(captured_args.contains("--max-redirs\n0\n"));
    assert_eq!(
        fs::read(FileSlotStorage::new(&root.0).image_path(Slot::B)).unwrap(),
        payload
    );
    assert_eq!(boot.state().unwrap().trial.unwrap().version, "1.1.0");
}

#[test]
fn install_url_requires_client_certificate_and_key_as_a_pair() {
    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install-url",
            "--state-root",
            "/tmp/sdv-fota-unused",
            "--url",
            "https://updates.example.test/vehicle.sdvf",
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--client-cert",
            "/etc/sdv-fota/client.crt",
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("InvalidClientCredentials"));
}

#[test]
fn install_url_rejects_group_or_world_accessible_client_keys() {
    let root = TempRoot::new();
    let certificate = root.0.join("client.crt");
    fs::write(&certificate, b"test certificate").unwrap();
    fs::set_permissions(&certificate, fs::Permissions::from_mode(0o644)).unwrap();
    let private_key = root.0.join("client.key");
    fs::write(&private_key, b"test private key").unwrap();
    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o644)).unwrap();

    let fake_bin = root.0.join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let curl_called = root.0.join("curl-called");
    write_executable(
        fake_bin.join("curl").as_path(),
        "#!/bin/sh\ntouch \"$SDV_FOTA_TEST_CURL_CALLED\"\n",
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var_os("PATH")
            .unwrap_or_default()
            .to_string_lossy()
    );
    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install-url",
            "--state-root",
            "/tmp/sdv-fota-unused",
            "--url",
            "https://updates.example.test/vehicle.sdvf",
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--client-cert",
            certificate.to_str().unwrap(),
            "--client-key",
            private_key.to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("SDV_FOTA_TEST_CURL_CALLED", &curl_called)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("UnsafeClientCredentials"));
    assert!(!curl_called.exists());
}
#[test]
fn install_url_rejects_credentials_beneath_a_writable_ancestor() {
    let root = TempRoot::new();
    let writable_ancestor = root.0.join("writable-ancestor");
    fs::create_dir(&writable_ancestor).unwrap();
    fs::set_permissions(&writable_ancestor, fs::Permissions::from_mode(0o777)).unwrap();
    let private_directory = writable_ancestor.join("private");
    fs::create_dir(&private_directory).unwrap();
    fs::set_permissions(&private_directory, fs::Permissions::from_mode(0o700)).unwrap();

    let certificate = private_directory.join("client.crt");
    fs::write(&certificate, b"test certificate").unwrap();
    fs::set_permissions(&certificate, fs::Permissions::from_mode(0o644)).unwrap();
    let private_key = private_directory.join("client.key");
    fs::write(&private_key, b"test private key").unwrap();
    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o600)).unwrap();

    let result = HttpsPackageSource::open_with_client_identity(
        "https://updates.example.test/vehicle.sdvf",
        Some(&certificate),
        Some(&private_key),
    );

    assert!(matches!(
        result,
        Err(sdv_fota::linux::Error::UnsafeClientCredentials)
    ));
}

#[test]
fn install_url_rejects_symlinked_credential_ancestors_and_parent_components() {
    let root = TempRoot::new();
    let private_directory = root.0.join("private");
    fs::create_dir(&private_directory).unwrap();
    fs::set_permissions(&private_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let certificate = private_directory.join("client.crt");
    fs::write(&certificate, b"test certificate").unwrap();
    fs::set_permissions(&certificate, fs::Permissions::from_mode(0o644)).unwrap();
    let private_key = private_directory.join("client.key");
    fs::write(&private_key, b"test private key").unwrap();
    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o600)).unwrap();

    let directory_link = root.0.join("credential-link");
    std::os::unix::fs::symlink(&private_directory, &directory_link).unwrap();
    let through_link = HttpsPackageSource::open_with_client_identity(
        "https://updates.example.test/vehicle.sdvf",
        Some(&directory_link.join("client.crt")),
        Some(&directory_link.join("client.key")),
    );
    assert!(matches!(
        through_link,
        Err(sdv_fota::linux::Error::UnsafeClientCredentials)
    ));

    let parent_component = private_directory.join("../private/client.crt");
    let parent_component_key = private_directory.join("../private/client.key");
    let through_parent_component = HttpsPackageSource::open_with_client_identity(
        "https://updates.example.test/vehicle.sdvf",
        Some(&parent_component),
        Some(&parent_component_key),
    );
    assert!(matches!(
        through_parent_component,
        Err(sdv_fota::linux::Error::UnsafeClientCredentials)
    ));
}

#[test]
fn failed_https_transfer_aborts_candidate_and_does_not_schedule_trial() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let payload = b"network transfer interrupted";
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": "vehicle",
            "swc": "VehicleControlApp",
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
    let package_path = root.0.join("partial-network-response.sdvf");
    fs::write(&package_path, package).unwrap();
    let trust_store = write_trust_store(&root);

    let fake_bin = root.0.join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    write_executable(
        fake_bin.join("curl").as_path(),
        "#!/bin/sh\ncat \"$SDV_FOTA_TEST_PACKAGE\"\nexit 22\n",
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var_os("PATH")
            .unwrap_or_default()
            .to_string_lossy()
    );
    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install-url",
            "--state-root",
            root.0.to_str().unwrap(),
            "--url",
            "https://updates.example.test/vehicle.sdvf",
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--trial-boots",
            "2",
        ])
        .env("PATH", path)
        .env("SDV_FOTA_TEST_PACKAGE", &package_path)
        .env("SDV_FOTA_TEST_TRUST_STORE", trust_store)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(boot.state().unwrap().trial.is_none());
    let inactive = root.0.join("slots/b");
    assert!(fs::read_dir(inactive).unwrap().next().is_none());
}

#[cfg(feature = "host-signing")]
#[test]
fn signing_command_rejects_metadata_the_installer_will_not_accept() {
    let root = TempRoot::new();
    let seed_path = root.0.join("signing.seed");
    write_seed_file(&seed_path, &SIGNING_SEED, 0o600);
    let image_path = root.0.join("candidate.bin");
    fs::write(&image_path, b"candidate image").unwrap();

    for (index, (key_id, version, config_schema, signal_if_version)) in [
        ("bad$key", "1.1.0", "1", "1"),
        (KEY_ID, "1.0", "1", "1"),
        (KEY_ID, "1.1.0", "4294967296", "1"),
        (KEY_ID, "1.1.0", "1", "4294967296"),
    ]
    .into_iter()
    .enumerate()
    {
        let package_path = root.0.join(format!("invalid-{index}.sdvf"));
        let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota-sign"))
            .args([
                "--seed",
                seed_path.to_str().unwrap(),
                "--key-id",
                key_id,
                "--target",
                "vehicle",
                "--version",
                version,
                "--security-version",
                "2",
                "--trial-ms",
                "5000",
                "--config-schema",
                config_schema,
                "--signal-if-version",
                signal_if_version,
                "--image",
                image_path.to_str().unwrap(),
                "--output",
                package_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        assert!(!result.status.success());
        assert!(!package_path.exists());
    }
}

#[cfg(feature = "host-signing")]
#[test]
fn signing_command_rejects_group_or_world_accessible_seed_files() {
    let root = TempRoot::new();
    let seed_path = root.0.join("signing.seed");
    write_seed_file(&seed_path, &SIGNING_SEED, 0o644);
    let image_path = root.0.join("candidate.bin");
    fs::write(&image_path, b"candidate image").unwrap();
    let package_path = root.0.join("candidate.sdvf");

    let result = Command::new(env!("CARGO_BIN_EXE_sdv-fota-sign"))
        .args([
            "--seed",
            seed_path.to_str().unwrap(),
            "--key-id",
            KEY_ID,
            "--target",
            "vehicle",
            "--version",
            "1.1.0",
            "--security-version",
            "2",
            "--trial-ms",
            "5000",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--image",
            image_path.to_str().unwrap(),
            "--output",
            package_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("group or other users"));
    assert!(!package_path.exists());
}

#[cfg(feature = "host-signing")]
#[test]
fn signing_and_install_commands_complete_a_host_update() {
    let root = TempRoot::new();
    let boot = FileBootControl::new(&root.0);
    boot.initialize(Slot::A, "1.0.0", 1).unwrap();
    let seed_path = root.0.join("signing.seed");
    write_seed_file(&seed_path, &SIGNING_SEED, 0o600);
    let image_path = root.0.join("candidate.bin");
    let payload = b"#!/bin/sh\nexec sleep 3\n";
    fs::write(&image_path, payload).unwrap();
    let package_path = root.0.join("candidate.sdvf");

    let signed = Command::new(env!("CARGO_BIN_EXE_sdv-fota-sign"))
        .args([
            "--seed",
            seed_path.to_str().unwrap(),
            "--key-id",
            KEY_ID,
            "--target",
            "vehicle",
            "--version",
            "1.1.0",
            "--security-version",
            "2",
            "--trial-ms",
            "5000",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--image",
            image_path.to_str().unwrap(),
            "--output",
            package_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );

    let trust_store = write_trust_store(&root);
    let installed = Command::new(env!("CARGO_BIN_EXE_sdv-fota"))
        .args([
            "install",
            "--state-root",
            root.0.to_str().unwrap(),
            "--package",
            package_path.to_str().unwrap(),
            "--target",
            "vehicle",
            "--config-schema",
            "1",
            "--signal-if-version",
            "1",
            "--trial-boots",
            "2",
        ])
        .env("SDV_FOTA_TEST_TRUST_STORE", trust_store)
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
