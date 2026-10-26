use sdv_fota::{
    encode_package, parse_package, resolve_trial, validate_package, BootControl, BootDecision,
    BootState, Ecu, FotaInstaller, HealthMonitor, ImageStorage, InstallError, Slot, TrialImage,
    TrustKey, ValidationContext, ValidationError,
};
use sdv_sec::{ed25519_public_key, ed25519_sign, sha256};
use std::convert::Infallible;

const KEY_ID: &str = "fota-test";
const SEED: [u8; 32] = [0x42; 32];
const PAYLOAD: &[u8] = b"new ecu image";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn signed_package(version: &str, security_version: u64) -> Vec<u8> {
    let manifest = format!(
        r#"{{
          "manifest_schema": 1,
          "image": {{
            "target_ecu": "engine",
            "swc": "EngineSWC",
            "version": "{version}",
            "security_version": {security_version},
            "size_bytes": {},
            "sha256": "{}"
          }},
          "requires": {{"signal_if_version": 1, "config_schema": [1]}},
          "policy": {{"allow_downgrade": false, "trial_ms": 5000}}
        }}"#,
        PAYLOAD.len(),
        hex(&sha256(PAYLOAD)),
    );
    let signature = ed25519_sign(&SEED, manifest.as_bytes());
    encode_package(KEY_ID, &signature, manifest.as_bytes(), PAYLOAD).unwrap()
}

fn validation_context<'a>() -> ValidationContext<'a> {
    ValidationContext {
        target_ecu: Ecu::Engine,
        current_version: "1.0.0",
        highest_security_version: 1,
        config_schema: 1,
        signal_if_version: 1,
    }
}

#[derive(Default)]
struct MemoryImageStorage {
    images: [Vec<u8>; 2],
}

impl ImageStorage for MemoryImageStorage {
    type Error = Infallible;

    fn write_inactive_image(&mut self, active: Slot, image: &[u8]) -> Result<Slot, Self::Error> {
        let target = active.other();
        self.images[slot_index(target)] = image.to_vec();
        Ok(target)
    }

    fn read_image(&self, slot: Slot) -> Result<Vec<u8>, Self::Error> {
        Ok(self.images[slot_index(slot)].clone())
    }
}

struct MemoryBootControl {
    state: BootState,
}

impl MemoryBootControl {
    fn current() -> Self {
        Self {
            state: BootState {
                active: Slot::A,
                active_version: "1.0.0".into(),
                highest_security_version: 1,
                trial: None,
            },
        }
    }
}

impl BootControl for MemoryBootControl {
    type Error = Infallible;

    fn state(&self) -> Result<BootState, Self::Error> {
        Ok(self.state.clone())
    }

    fn schedule_trial(&mut self, trial: TrialImage) -> Result<(), Self::Error> {
        self.state.trial = Some(trial);
        Ok(())
    }

    fn select_for_boot(&mut self) -> Result<BootDecision, Self::Error> {
        Ok(match &self.state.trial {
            Some(trial) => BootDecision {
                slot: trial.slot,
                trial: true,
            },
            None => BootDecision {
                slot: self.state.active,
                trial: false,
            },
        })
    }

    fn commit_trial(&mut self) -> Result<(), Self::Error> {
        if let Some(trial) = self.state.trial.take() {
            self.state.active = trial.slot;
            self.state.active_version = trial.version;
            self.state.highest_security_version = self
                .state
                .highest_security_version
                .max(trial.security_version);
        }
        Ok(())
    }

    fn rollback_trial(&mut self) -> Result<(), Self::Error> {
        self.state.trial = None;
        Ok(())
    }
}

struct FixedHealth(bool);

impl HealthMonitor for FixedHealth {
    type Error = Infallible;

    fn check(&mut self, _slot: Slot, _trial_window_ms: u32) -> Result<bool, Self::Error> {
        Ok(self.0)
    }
}

fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::A => 0,
        Slot::B => 1,
    }
}

#[test]
fn signed_package_is_verified_and_compatible() {
    let bytes = signed_package("1.1.0", 2);
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let package = parse_package(&bytes).unwrap();
    let validated = validate_package(&package, &[trust_key], &validation_context()).unwrap();

    assert_eq!(validated.manifest().image.version, "1.1.0");
    assert_eq!(validated.payload(), PAYLOAD);
}

#[test]
fn invalid_signature_is_rejected_before_manifest_use() {
    let mut bytes = signed_package("1.1.0", 2);
    bytes[18] ^= 0x01;
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let package = parse_package(&bytes).unwrap();

    assert!(matches!(
        validate_package(&package, &[trust_key], &validation_context()),
        Err(ValidationError::InvalidSignature)
    ));
}

#[test]
fn installer_stages_inactive_image_and_schedules_trial() {
    let bytes = signed_package("1.1.0", 2);
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };

    let slot = installer
        .stage_package(&bytes, &[trust_key], &validation_context())
        .unwrap();

    assert_eq!(slot, Slot::B);
    assert_eq!(installer.storage.images[1], PAYLOAD);
    let trial = installer.boot_control.state.trial.as_ref().unwrap();
    assert_eq!(trial.slot, Slot::B);
    assert_eq!(trial.version, "1.1.0");
    assert_eq!(trial.boots_left, 2);
}

#[test]
fn downgrade_is_rejected_without_writing_or_scheduling_trial() {
    let bytes = signed_package("1.0.0", 2);
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };

    let result = installer.stage_package(&bytes, &[trust_key], &validation_context());

    assert!(matches!(
        result,
        Err(InstallError::Validation(ValidationError::Downgrade))
    ));
    assert!(installer.storage.images.iter().all(Vec::is_empty));
    assert!(installer.boot_control.state.trial.is_none());
}

#[test]
fn failed_health_check_rolls_back_trial() {
    let mut boot = MemoryBootControl::current();
    boot.schedule_trial(TrialImage {
        slot: Slot::B,
        version: "1.1.0".into(),
        security_version: 2,
        boots_left: 1,
    })
    .unwrap();

    let result = resolve_trial(&mut boot, &mut FixedHealth(false), 5000).unwrap();

    assert_eq!(result, sdv_fota::TrialResolution::RolledBack);
    assert_eq!(boot.state.active, Slot::A);
    assert!(boot.state.trial.is_none());
}
