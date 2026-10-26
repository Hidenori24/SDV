//! Service-level ports between FOTA policy and platform services.
//!
//! These traits are not AUTOSAR APIs. A Classic integration maps them to the selected
//! DCM/Csm/NvM/MemIf/Fls and bootloader services; the Linux adapter remains the Pi 5 host backend.

use crate::{BootDecision, BootState, Slot, TrialImage};
use alloc::vec::Vec;

/// Service-level image partition port.
///
/// A Classic integration must implement this contract through the selected memory stack
/// (typically ECU abstraction / MemIf / Fls); this trait is not a replacement for those APIs.
pub trait ImageStorage {
    type Error;

    /// Write to the slot opposite `active` and return the slot that was written.
    fn write_inactive_image(&mut self, active: Slot, image: &[u8]) -> Result<Slot, Self::Error>;
    fn read_image(&self, slot: Slot) -> Result<Vec<u8>, Self::Error>;
}

/// Transactional update-state and activation port.
///
/// A Classic adapter composes persistent metadata (typically NvM) with the OEM boot manager or
/// bootloader contract. It must preserve the trial/commit/rollback guarantees across reset.
pub trait BootControl {
    type Error;

    fn state(&self) -> Result<BootState, Self::Error>;
    fn schedule_trial(&mut self, trial: TrialImage) -> Result<(), Self::Error>;
    fn select_for_boot(&mut self) -> Result<BootDecision, Self::Error>;
    fn commit_trial(&mut self) -> Result<(), Self::Error>;
    fn rollback_trial(&mut self) -> Result<(), Self::Error>;
}

/// Checks whether a candidate process or ECU has met its platform health criteria.
///
/// This is an OEM health policy boundary; it is not a standardized AUTOSAR service by itself.
pub trait HealthMonitor {
    type Error;

    fn check(&mut self, slot: Slot, trial_window_ms: u32) -> Result<bool, Self::Error>;
}

/// Acquires one complete package from a host transport or a Classic diagnostic integration.
///
/// A DCM-backed implementation owns session/download framing and forwards validated byte ranges
/// to the FOTA service. This whole-package convenience API is currently host-oriented.
pub trait PackageSource {
    type Error;

    fn receive_package(&mut self, max_bytes: usize) -> Result<Vec<u8>, Self::Error>;
}
