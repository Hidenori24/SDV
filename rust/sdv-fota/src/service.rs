//! Platform-independent FOTA update service and trial resolution.

use crate::package::{
    decode_sha256, parse_package, parse_version, validate_package, validate_signed_manifest,
};
use crate::{
    BootControl, HealthMonitor, ImageStorage, PackageSource, Slot, TrialImage, TrustKey,
    ValidatedPackage, ValidationContext, ValidationError,
};
use sdv_sec::Sha256Context;

const IMAGE_IO_CHUNK_BYTES: usize = 1024;

pub struct FotaInstaller<S, B> {
    pub storage: S,
    pub boot_control: B,
    pub trial_boots: u32,
}

#[derive(Debug)]
pub enum InstallError<StorageError, BootError> {
    Validation(ValidationError),
    Storage(StorageError),
    BootControl(BootError),
    TrialAlreadyPending,
    ImageTooLarge,
    Downgrade,
    SecurityVersionRollback,
    ReadbackMismatch,
}

#[derive(Debug)]
pub enum StreamInstallError<SourceError, StorageError, BootError> {
    Source(SourceError),
    Validation(ValidationError),
    Storage(StorageError),
    BootControl(BootError),
    TrialAlreadyPending,
    Downgrade,
    SecurityVersionRollback,
    ImageTooLarge,
    ReadbackMismatch,
}

impl<S: ImageStorage, B: BootControl> FotaInstaller<S, B> {
    /// Parse and validate a signed package, then write it to the inactive slot.
    pub fn stage_package(
        &mut self,
        package_bytes: &[u8],
        trust_keys: &[TrustKey<'_>],
        context: &ValidationContext<'_>,
    ) -> Result<Slot, InstallError<S::Error, B::Error>> {
        let package = parse_package(package_bytes).map_err(InstallError::Validation)?;
        let validated =
            validate_package(&package, trust_keys, context).map_err(InstallError::Validation)?;
        self.stage_validated(&validated)
    }

    /// Write a previously signature- and compatibility-validated image to the inactive slot,
    /// read it back, then ask the boot-control adapter to schedule a bounded trial boot.
    pub fn stage_validated(
        &mut self,
        package: &ValidatedPackage<'_>,
    ) -> Result<Slot, InstallError<S::Error, B::Error>> {
        let state = self
            .boot_control
            .state()
            .map_err(InstallError::BootControl)?;
        if state.trial.is_some() {
            return Err(InstallError::TrialAlreadyPending);
        }
        let manifest = package.manifest();
        let incoming = parse_version(&manifest.image.version)
            .ok_or(InstallError::Validation(ValidationError::InvalidVersion))?;
        let current = parse_version(&state.active_version)
            .ok_or(InstallError::Validation(ValidationError::InvalidVersion))?;
        if !manifest.policy.allow_downgrade && incoming <= current {
            return Err(InstallError::Downgrade);
        }
        let security_version = manifest.image.security_version;
        if security_version < state.highest_security_version {
            return Err(InstallError::SecurityVersionRollback);
        }
        let image_size =
            u32::try_from(package.payload().len()).map_err(|_| InstallError::ImageTooLarge)?;
        let slot = self
            .storage
            .begin_inactive_image(state.active, image_size)
            .map_err(InstallError::Storage)?;
        let mut offset = 0u32;
        for chunk in package.payload().chunks(IMAGE_IO_CHUNK_BYTES) {
            if let Err(error) = self.storage.write_image_chunk(slot, offset, chunk) {
                self.storage.abort_inactive_image(slot);
                return Err(InstallError::Storage(error));
            }
            offset += chunk.len() as u32;
        }
        if let Err(error) = self.storage.finish_inactive_image(slot) {
            self.storage.abort_inactive_image(slot);
            return Err(InstallError::Storage(error));
        }
        let expected =
            decode_sha256(&manifest.image.sha256).ok_or(InstallError::ReadbackMismatch)?;
        let mut digest = Sha256Context::new();
        let mut readback = [0u8; IMAGE_IO_CHUNK_BYTES];
        let mut offset = 0u32;
        while offset < image_size {
            let requested = (image_size - offset).min(IMAGE_IO_CHUNK_BYTES as u32) as usize;
            let read = self
                .storage
                .read_image_chunk(slot, offset, &mut readback[..requested])
                .map_err(InstallError::Storage)?;
            if read == 0 || read > requested {
                return Err(InstallError::ReadbackMismatch);
            }
            digest.update(&readback[..read]);
            offset += read as u32;
        }
        if digest.finalize() != expected {
            return Err(InstallError::ReadbackMismatch);
        }
        self.boot_control
            .schedule_trial(TrialImage {
                slot,
                version: manifest.image.version.clone(),
                security_version,
                boots_left: self.trial_boots.max(1),
            })
            .map_err(InstallError::BootControl)?;
        Ok(slot)
    }

    /// Validate a signed manifest, then stream its payload into the inactive image slot.
    ///
    /// The caller owns the bounded manifest buffer and transport framing. Payload transfer,
    /// flash writing, and read-back hashing use fixed-size buffers rather than an image-sized
    /// allocation. `source.finish()` must reject trailing or incomplete transport data.
    pub fn stage_signed_manifest_from_source<P: PackageSource>(
        &mut self,
        key_id: &str,
        signature: &[u8],
        manifest_bytes: &[u8],
        payload_size: u64,
        source: &mut P,
        trust_keys: &[TrustKey<'_>],
        context: &ValidationContext<'_>,
    ) -> Result<Slot, StreamInstallError<P::Error, S::Error, B::Error>> {
        let manifest = validate_signed_manifest(
            key_id,
            signature,
            manifest_bytes,
            payload_size,
            trust_keys,
            context,
        )
        .map_err(StreamInstallError::Validation)?;
        let state = self
            .boot_control
            .state()
            .map_err(StreamInstallError::BootControl)?;
        if state.trial.is_some() {
            return Err(StreamInstallError::TrialAlreadyPending);
        }
        let incoming = parse_version(&manifest.image.version).ok_or(
            StreamInstallError::Validation(ValidationError::InvalidVersion),
        )?;
        let current = parse_version(&state.active_version).ok_or(
            StreamInstallError::Validation(ValidationError::InvalidVersion),
        )?;
        if !manifest.policy.allow_downgrade && incoming <= current {
            return Err(StreamInstallError::Downgrade);
        }
        let security_version = manifest.image.security_version;
        if security_version < state.highest_security_version {
            return Err(StreamInstallError::SecurityVersionRollback);
        }
        let image_size =
            u32::try_from(payload_size).map_err(|_| StreamInstallError::ImageTooLarge)?;
        let expected = decode_sha256(&manifest.image.sha256).ok_or(
            StreamInstallError::Validation(ValidationError::InvalidSha256),
        )?;
        let slot = self
            .storage
            .begin_inactive_image(state.active, image_size)
            .map_err(StreamInstallError::Storage)?;

        let mut offset = 0u32;
        let mut digest = Sha256Context::new();
        let mut chunk = [0u8; IMAGE_IO_CHUNK_BYTES];
        while offset < image_size {
            let chunk_len = (image_size - offset).min(IMAGE_IO_CHUNK_BYTES as u32) as usize;
            if let Err(error) = source.read_exact(&mut chunk[..chunk_len]) {
                self.storage.abort_inactive_image(slot);
                return Err(StreamInstallError::Source(error));
            }
            digest.update(&chunk[..chunk_len]);
            if let Err(error) = self
                .storage
                .write_image_chunk(slot, offset, &chunk[..chunk_len])
            {
                self.storage.abort_inactive_image(slot);
                return Err(StreamInstallError::Storage(error));
            }
            offset += chunk_len as u32;
        }
        if let Err(error) = source.finish() {
            self.storage.abort_inactive_image(slot);
            return Err(StreamInstallError::Source(error));
        }
        if digest.finalize() != expected {
            self.storage.abort_inactive_image(slot);
            return Err(StreamInstallError::Validation(
                ValidationError::PayloadHashMismatch,
            ));
        }
        if let Err(error) = self.storage.finish_inactive_image(slot) {
            self.storage.abort_inactive_image(slot);
            return Err(StreamInstallError::Storage(error));
        }

        let mut readback_digest = Sha256Context::new();
        let mut readback = [0u8; IMAGE_IO_CHUNK_BYTES];
        let mut read_offset = 0u32;
        while read_offset < image_size {
            let requested = (image_size - read_offset).min(IMAGE_IO_CHUNK_BYTES as u32) as usize;
            let read = self
                .storage
                .read_image_chunk(slot, read_offset, &mut readback[..requested])
                .map_err(StreamInstallError::Storage)?;
            if read == 0 || read > requested {
                return Err(StreamInstallError::ReadbackMismatch);
            }
            readback_digest.update(&readback[..read]);
            read_offset += read as u32;
        }
        if readback_digest.finalize() != expected {
            return Err(StreamInstallError::ReadbackMismatch);
        }
        self.boot_control
            .schedule_trial(TrialImage {
                slot,
                version: manifest.image.version,
                security_version,
                boots_left: self.trial_boots.max(1),
            })
            .map_err(StreamInstallError::BootControl)?;
        Ok(slot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialResolution {
    Committed,
    RolledBack,
}

#[derive(Debug)]
pub enum TrialError<BootError, HealthError> {
    BootControl(BootError),
    HealthMonitor(HealthError),
    NoTrialPending,
}

/// Resolve the pending trial using a platform health monitor. A failed or unavailable health
/// check never commits the image; it requests rollback instead.
pub fn resolve_trial<B: BootControl, H: HealthMonitor>(
    boot: &mut B,
    health: &mut H,
    trial_window_ms: u32,
) -> Result<TrialResolution, TrialError<B::Error, H::Error>> {
    let state = boot.state().map_err(TrialError::BootControl)?;
    let trial = state.trial.ok_or(TrialError::NoTrialPending)?;
    let healthy = match health.check(trial.slot, trial_window_ms) {
        Ok(healthy) => healthy,
        Err(error) => {
            boot.rollback_trial().map_err(TrialError::BootControl)?;
            return Err(TrialError::HealthMonitor(error));
        }
    };
    if healthy {
        boot.commit_trial().map_err(TrialError::BootControl)?;
        Ok(TrialResolution::Committed)
    } else {
        boot.rollback_trial().map_err(TrialError::BootControl)?;
        Ok(TrialResolution::RolledBack)
    }
}
