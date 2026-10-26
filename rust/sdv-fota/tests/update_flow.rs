use sdv_fota::{
    encode_package, parse_package, resolve_trial, validate_package, BootControl, BootDecision,
    BootState, Ecu, FotaInstaller, HealthMonitor, ImageStorage, InstallError, PackageSource, Slot,
    TrialImage, TrustKey, ValidationContext, ValidationError,
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
    signed_package_for_payload(version, security_version, PAYLOAD)
}

fn signed_package_for_payload(version: &str, security_version: u64, payload: &[u8]) -> Vec<u8> {
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
        payload.len(),
        hex(&sha256(payload)),
    );
    let signature = ed25519_sign(&SEED, manifest.as_bytes());
    encode_package(KEY_ID, &signature, manifest.as_bytes(), payload).unwrap()
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
    pending: Option<(Slot, u32, Vec<u8>)>,
}

impl ImageStorage for MemoryImageStorage {
    type Error = Infallible;

    fn begin_inactive_image(&mut self, active: Slot, image_size: u32) -> Result<Slot, Self::Error> {
        let target = active.other();
        self.pending = Some((target, image_size, Vec::with_capacity(image_size as usize)));
        Ok(target)
    }

    fn write_image_chunk(
        &mut self,
        slot: Slot,
        offset: u32,
        chunk: &[u8],
    ) -> Result<(), Self::Error> {
        let (pending_slot, expected_size, image) = self.pending.as_mut().unwrap();
        assert_eq!(*pending_slot, slot);
        assert_eq!(offset as usize, image.len());
        image.extend_from_slice(chunk);
        assert!(image.len() <= *expected_size as usize);
        Ok(())
    }

    fn finish_inactive_image(&mut self, slot: Slot) -> Result<(), Self::Error> {
        let (pending_slot, expected_size, image) = self.pending.take().unwrap();
        assert_eq!(pending_slot, slot);
        assert_eq!(image.len(), expected_size as usize);
        self.images[slot_index(slot)] = image;
        Ok(())
    }

    fn abort_inactive_image(&mut self, slot: Slot) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending_slot, _, _)| *pending_slot == slot)
        {
            self.pending = None;
        }
    }

    fn read_image_chunk(
        &self,
        slot: Slot,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, Self::Error> {
        let image = &self.images[slot_index(slot)];
        let start = offset as usize;
        if start >= image.len() {
            return Ok(0);
        }
        let length = buffer.len().min(image.len() - start);
        buffer[..length].copy_from_slice(&image[start..start + length]);
        Ok(length)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceFailure {
    UnexpectedEnd,
    TrailingData,
}

struct SlicePackageSource<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl PackageSource for SlicePackageSource<'_> {
    type Error = SourceFailure;

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Self::Error> {
        let end = self
            .offset
            .checked_add(buffer.len())
            .ok_or(SourceFailure::UnexpectedEnd)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(SourceFailure::UnexpectedEnd)?;
        buffer.copy_from_slice(bytes);
        self.offset = end;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(SourceFailure::TrailingData)
        }
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
fn manifest_fields_have_fixed_capacity() {
    let overlong_version = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": "engine",
            "swc": "EngineSWC",
            "version": "123456789012345678901234567890123",
            "security_version": 2,
            "size_bytes": 1,
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
        },
        "requires": {"signal_if_version": 1, "config_schema": [1]},
        "policy": {"allow_downgrade": false, "trial_ms": 1000}
    });
    assert!(serde_json::from_value::<sdv_fota::Manifest>(overlong_version.clone()).is_err());

    let mut excessive_schemas = overlong_version;
    excessive_schemas["image"]["version"] = serde_json::json!("1.2.3");
    excessive_schemas["requires"]["config_schema"] =
        serde_json::json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]);
    assert!(serde_json::from_value::<sdv_fota::Manifest>(excessive_schemas).is_err());
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
fn installer_writes_and_verifies_image_across_multiple_flash_blocks() {
    let payload = vec![0xA5; 2501];
    let bytes = signed_package_for_payload("1.1.0", 2, &payload);
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
    assert_eq!(installer.storage.images[1], payload);
    assert!(installer.boot_control.state.trial.is_some());
}

#[test]
fn streamed_package_payload_is_written_without_a_whole_image_readback() {
    let payload = vec![0x3C; 2501];
    let bytes = signed_package_for_payload("1.1.0", 2, &payload);
    let package = parse_package(&bytes).unwrap();
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let mut source = SlicePackageSource {
        bytes: package.payload,
        offset: 0,
    };
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };

    let slot = installer
        .stage_signed_manifest_from_source(
            package.key_id,
            package.signature,
            package.manifest_bytes,
            package.payload.len() as u64,
            &mut source,
            &[trust_key],
            &validation_context(),
        )
        .unwrap();

    assert_eq!(slot, Slot::B);
    assert_eq!(installer.storage.images[1], payload);
    assert!(installer.boot_control.state.trial.is_some());
}

#[test]
fn framed_package_stream_uses_caller_manifest_buffer() {
    let payload = vec![0xC3; 2200];
    let bytes = signed_package_for_payload("1.1.0", 2, &payload);
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let mut source = SlicePackageSource {
        bytes: &bytes,
        offset: 0,
    };
    let mut manifest_buffer = [0u8; sdv_fota::MAX_MANIFEST_BYTES];
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };

    let slot = installer
        .stage_package_from_source(
            &mut source,
            &mut manifest_buffer,
            &[trust_key],
            &validation_context(),
        )
        .unwrap();

    assert_eq!(slot, Slot::B);
    assert_eq!(installer.storage.images[1], payload);
    assert!(installer.boot_control.state.trial.is_some());
}

#[test]
fn streamed_payload_hash_failure_aborts_inactive_image() {
    let mut bytes = signed_package("1.1.0", 2);
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let package = parse_package(&bytes).unwrap();
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };
    let mut source = SlicePackageSource {
        bytes: package.payload,
        offset: 0,
    };
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };

    let result = installer.stage_signed_manifest_from_source(
        package.key_id,
        package.signature,
        package.manifest_bytes,
        package.payload.len() as u64,
        &mut source,
        &[trust_key],
        &validation_context(),
    );

    assert!(matches!(
        result,
        Err(sdv_fota::StreamInstallError::Validation(
            ValidationError::PayloadHashMismatch
        ))
    ));
    assert!(installer.storage.images.iter().all(Vec::is_empty));
    assert!(installer.storage.pending.is_none());
    assert!(installer.boot_control.state.trial.is_none());
}

#[test]
fn streamed_payload_rejects_truncation_and_trailing_bytes() {
    let payload = vec![0x5A; 1500];
    let bytes = signed_package_for_payload("1.1.0", 2, &payload);
    let package = parse_package(&bytes).unwrap();
    let public_key = ed25519_public_key(&SEED);
    let trust_key = TrustKey {
        key_id: KEY_ID,
        public_key: &public_key,
    };

    let mut truncated = SlicePackageSource {
        bytes: &package.payload[..package.payload.len() - 1],
        offset: 0,
    };
    let mut installer = FotaInstaller {
        storage: MemoryImageStorage::default(),
        boot_control: MemoryBootControl::current(),
        trial_boots: 2,
    };
    let truncated_result = installer.stage_signed_manifest_from_source(
        package.key_id,
        package.signature,
        package.manifest_bytes,
        package.payload.len() as u64,
        &mut truncated,
        &[trust_key],
        &validation_context(),
    );
    assert!(matches!(
        truncated_result,
        Err(sdv_fota::StreamInstallError::Source(
            SourceFailure::UnexpectedEnd
        ))
    ));
    assert!(installer.storage.images.iter().all(Vec::is_empty));

    let mut with_trailing_byte = package.payload.to_vec();
    with_trailing_byte.push(0xFF);
    let mut trailing = SlicePackageSource {
        bytes: &with_trailing_byte,
        offset: 0,
    };
    let trailing_result = installer.stage_signed_manifest_from_source(
        package.key_id,
        package.signature,
        package.manifest_bytes,
        package.payload.len() as u64,
        &mut trailing,
        &[TrustKey {
            key_id: KEY_ID,
            public_key: &public_key,
        }],
        &validation_context(),
    );
    assert!(matches!(
        trailing_result,
        Err(sdv_fota::StreamInstallError::Source(
            SourceFailure::TrailingData
        ))
    ));
    assert!(installer.storage.images.iter().all(Vec::is_empty));
    assert!(installer.boot_control.state.trial.is_none());
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
        trial_window_ms: 5_000,
    })
    .unwrap();

    let result = resolve_trial(&mut boot, &mut FixedHealth(false), 5000).unwrap();

    assert_eq!(result, sdv_fota::TrialResolution::RolledBack);
    assert_eq!(boot.state.active, Slot::A);
    assert!(boot.state.trial.is_none());
}
