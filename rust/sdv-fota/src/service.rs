//! Platform-independent FOTA update service and trial resolution.

use crate::package::{decode_sha256, parse_package, parse_version, validate_package};
use crate::{
    BootControl, HealthMonitor, ImageStorage, Slot, TrialImage, TrustKey, ValidatedPackage,
    ValidationContext, ValidationError,
};
use sdv_sec::sha256;

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
    Downgrade,
    SecurityVersionRollback,
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
        let slot = self
            .storage
            .write_inactive_image(state.active, package.payload())
            .map_err(InstallError::Storage)?;
        let readback = self
            .storage
            .read_image(slot)
            .map_err(InstallError::Storage)?;
        let expected =
            decode_sha256(&manifest.image.sha256).ok_or(InstallError::ReadbackMismatch)?;
        if readback.len() != package.payload().len() || sha256(&readback) != expected {
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
