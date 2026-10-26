//! Synchronous C bridge for a target-owned Classic BSW/bootloader adapter.
//!
//! The bridge owns no platform state. Callbacks map to the selected ECU's diagnostic,
//! memory, NvM, crypto and boot-manager APIs. This is an SDV integration ABI, not AUTOSAR API.

use crate::{
    resolve_trial, BootControl, BootDecision, BootState, Ecu, FotaInstaller, HealthMonitor,
    ImageStorage, PackageSource, Slot, TrialImage, TrustKey, ValidationContext, VersionString,
    MAX_MANIFEST_BYTES,
};
use core::{ffi::c_void, slice};

pub const SDV_FOTA_OK: i32 = 0;
pub const SDV_FOTA_INVALID_ARG: i32 = -1;
pub const SDV_FOTA_CALLBACK_ERROR: i32 = 1;
pub const SDV_FOTA_REJECTED: i32 = 2;
pub const SDV_FOTA_UPDATE_PENDING: i32 = 3;
pub const SDV_FOTA_IO_ERROR: i32 = 4;

const VERSION_CAPACITY: usize = 32;

#[repr(C)]
pub struct SdvFotaState {
    pub active_slot: u8,
    pub active_version: [u8; VERSION_CAPACITY],
    pub active_version_len: usize,
    pub highest_security_version: u64,
    pub trial_pending: u8,
    pub trial_slot: u8,
    pub trial_version: [u8; VERSION_CAPACITY],
    pub trial_version_len: usize,
    pub trial_security_version: u64,
    pub trial_boots_left: u32,
    pub trial_window_ms: u32,
}

#[repr(C)]
pub struct SdvFotaTrial {
    pub slot: u8,
    pub version: [u8; VERSION_CAPACITY],
    pub version_len: usize,
    pub security_version: u64,
    pub boots_left: u32,
    pub trial_window_ms: u32,
}

#[repr(C)]
pub struct SdvFotaCallbacks {
    pub context: *mut c_void,
    pub source_read_exact: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> i32>,
    pub source_finish: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub read_state: Option<unsafe extern "C" fn(*mut c_void, *mut SdvFotaState) -> i32>,
    pub begin_image: Option<unsafe extern "C" fn(*mut c_void, u8, u32, *mut u8) -> i32>,
    pub write_image: Option<unsafe extern "C" fn(*mut c_void, u8, u32, *const u8, usize) -> i32>,
    pub finish_image: Option<unsafe extern "C" fn(*mut c_void, u8) -> i32>,
    pub abort_image: Option<unsafe extern "C" fn(*mut c_void, u8)>,
    pub read_image:
        Option<unsafe extern "C" fn(*mut c_void, u8, u32, *mut u8, usize, *mut usize) -> i32>,
    pub schedule_trial: Option<unsafe extern "C" fn(*mut c_void, *const SdvFotaTrial) -> i32>,
    pub select_for_boot: Option<unsafe extern "C" fn(*mut c_void, *mut u8, *mut u8) -> i32>,
    pub commit_trial: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub rollback_trial: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub check_health: Option<unsafe extern "C" fn(*mut c_void, u8, u32, *mut u8) -> i32>,
}

struct Source<'a>(&'a SdvFotaCallbacks);
impl PackageSource for Source<'_> {
    type Error = ();
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ()> {
        let callback = self.0.source_read_exact.ok_or(())?;
        // SAFETY: the caller provides a valid context and callback for this synchronous call.
        (unsafe { callback(self.0.context, buffer.as_mut_ptr(), buffer.len()) } == 0)
            .then_some(())
            .ok_or(())
    }
    fn finish(&mut self) -> Result<(), ()> {
        let callback = self.0.source_finish.ok_or(())?;
        // SAFETY: the caller provides a valid context and callback for this synchronous call.
        (unsafe { callback(self.0.context) } == 0)
            .then_some(())
            .ok_or(())
    }
}

struct Ports<'a>(&'a SdvFotaCallbacks);
impl ImageStorage for Ports<'_> {
    type Error = ();
    fn begin_inactive_image(&mut self, active: Slot, size: u32) -> Result<Slot, ()> {
        let callback = self.0.begin_image.ok_or(())?;
        let mut slot = 0xff;
        // SAFETY: callback is provided by the caller and writes one slot byte.
        if unsafe { callback(self.0.context, slot_id(active), size, &mut slot) } != 0 {
            return Err(());
        }
        slot_from_id(slot).ok_or(())
    }
    fn write_image_chunk(&mut self, slot: Slot, offset: u32, chunk: &[u8]) -> Result<(), ()> {
        let callback = self.0.write_image.ok_or(())?;
        // SAFETY: the input slice remains valid for the synchronous callback.
        (unsafe {
            callback(
                self.0.context,
                slot_id(slot),
                offset,
                chunk.as_ptr(),
                chunk.len(),
            )
        } == 0)
            .then_some(())
            .ok_or(())
    }
    fn finish_inactive_image(&mut self, slot: Slot) -> Result<(), ()> {
        let callback = self.0.finish_image.ok_or(())?;
        // SAFETY: callback/context are caller-owned and valid during this call.
        (unsafe { callback(self.0.context, slot_id(slot)) } == 0)
            .then_some(())
            .ok_or(())
    }
    fn abort_inactive_image(&mut self, slot: Slot) {
        if let Some(callback) = self.0.abort_image {
            // SAFETY: abort is best-effort and callback/context are caller-owned.
            unsafe { callback(self.0.context, slot_id(slot)) };
        }
    }
    fn read_image_chunk(&self, slot: Slot, offset: u32, buffer: &mut [u8]) -> Result<usize, ()> {
        let callback = self.0.read_image.ok_or(())?;
        let mut read = 0;
        // SAFETY: buffer and output length are valid for the synchronous callback.
        if unsafe {
            callback(
                self.0.context,
                slot_id(slot),
                offset,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut read,
            )
        } != 0
            || read > buffer.len()
        {
            return Err(());
        }
        Ok(read)
    }
}

impl BootControl for Ports<'_> {
    type Error = ();
    fn state(&self) -> Result<BootState, ()> {
        let callback = self.0.read_state.ok_or(())?;
        let mut state = SdvFotaState {
            active_slot: 0xff,
            active_version: [0; VERSION_CAPACITY],
            active_version_len: 0,
            highest_security_version: 0,
            trial_pending: 0,
            trial_slot: 0xff,
            trial_version: [0; VERSION_CAPACITY],
            trial_version_len: 0,
            trial_security_version: 0,
            trial_boots_left: 0,
            trial_window_ms: 0,
        };
        // SAFETY: callback fills the repr(C) state supplied by this function.
        if unsafe { callback(self.0.context, &mut state) } != 0
            || state.active_version_len > VERSION_CAPACITY
            || state.trial_version_len > VERSION_CAPACITY
            || state.trial_pending > 1
        {
            return Err(());
        }
        let active = slot_from_id(state.active_slot).ok_or(())?;
        let active_version = VersionString::try_from_str(
            core::str::from_utf8(&state.active_version[..state.active_version_len])
                .map_err(|_| ())?,
        )
        .ok_or(())?;
        let trial = if state.trial_pending == 1 {
            Some(TrialImage {
                slot: slot_from_id(state.trial_slot).ok_or(())?,
                version: VersionString::try_from_str(
                    core::str::from_utf8(&state.trial_version[..state.trial_version_len])
                        .map_err(|_| ())?,
                )
                .ok_or(())?,
                security_version: state.trial_security_version,
                boots_left: state.trial_boots_left,
                trial_window_ms: state.trial_window_ms,
            })
        } else {
            None
        };
        Ok(BootState {
            active,
            active_version,
            highest_security_version: state.highest_security_version,
            trial,
        })
    }
    fn schedule_trial(&mut self, trial: TrialImage) -> Result<(), ()> {
        let callback = self.0.schedule_trial.ok_or(())?;
        let bytes = trial.version.as_str().as_bytes();
        let mut version = [0; VERSION_CAPACITY];
        version[..bytes.len()].copy_from_slice(bytes);
        let c_trial = SdvFotaTrial {
            slot: slot_id(trial.slot),
            version,
            version_len: bytes.len(),
            security_version: trial.security_version,
            boots_left: trial.boots_left,
            trial_window_ms: trial.trial_window_ms,
        };
        // SAFETY: trial remains valid for the synchronous callback.
        (unsafe { callback(self.0.context, &c_trial) } == 0)
            .then_some(())
            .ok_or(())
    }
    fn select_for_boot(&mut self) -> Result<BootDecision, ()> {
        let callback = self.0.select_for_boot.ok_or(())?;
        let mut slot = 0xff;
        let mut trial = 0xff;
        // SAFETY: output bytes are valid for the synchronous callback.
        if unsafe { callback(self.0.context, &mut slot, &mut trial) } != 0 || trial > 1 {
            return Err(());
        }
        Ok(BootDecision {
            slot: slot_from_id(slot).ok_or(())?,
            trial: trial == 1,
        })
    }
    fn commit_trial(&mut self) -> Result<(), ()> {
        let callback = self.0.commit_trial.ok_or(())?;
        // SAFETY: callback/context are caller-owned and valid during this call.
        (unsafe { callback(self.0.context) } == 0)
            .then_some(())
            .ok_or(())
    }
    fn rollback_trial(&mut self) -> Result<(), ()> {
        let callback = self.0.rollback_trial.ok_or(())?;
        // SAFETY: callback/context are caller-owned and valid during this call.
        (unsafe { callback(self.0.context) } == 0)
            .then_some(())
            .ok_or(())
    }
}

struct Health<'a>(&'a SdvFotaCallbacks);
impl HealthMonitor for Health<'_> {
    type Error = ();
    fn check(&mut self, slot: Slot, trial_window_ms: u32) -> Result<bool, ()> {
        let callback = self.0.check_health.ok_or(())?;
        let mut healthy = 0xff;
        // SAFETY: output byte is valid for the synchronous callback.
        if unsafe { callback(self.0.context, slot_id(slot), trial_window_ms, &mut healthy) } != 0
            || healthy > 1
        {
            return Err(());
        }
        Ok(healthy == 1)
    }
}

fn slot_id(slot: Slot) -> u8 {
    match slot {
        Slot::A => 0,
        Slot::B => 1,
    }
}
fn slot_from_id(id: u8) -> Option<Slot> {
    match id {
        0 => Some(Slot::A),
        1 => Some(Slot::B),
        _ => None,
    }
}
fn ecu_from_id(id: u8) -> Option<Ecu> {
    match id {
        0 => Some(Ecu::Engine),
        1 => Some(Ecu::Brake),
        2 => Some(Ecu::Steering),
        _ => None,
    }
}

/// Stage one signed SDVF package through target supplied BSW callbacks.
///
/// `manifest_buffer` must hold at least `manifest_capacity` writable bytes; a 64 KiB buffer
/// is sufficient. `public_key` is exactly 32 bytes. Callback context and buffers must remain
/// valid for the duration of the call. This function is synchronous and does not retain pointers.
#[no_mangle]
pub unsafe extern "C" fn sdv_fota_stage_package(
    callbacks: *const SdvFotaCallbacks,
    key_id: *const u8,
    key_id_len: usize,
    public_key: *const u8,
    manifest_buffer: *mut u8,
    manifest_capacity: usize,
    target_ecu: u8,
    config_schema: u32,
    signal_if_version: u32,
) -> i32 {
    if callbacks.is_null()
        || key_id.is_null()
        || public_key.is_null()
        || manifest_buffer.is_null()
        || key_id_len == 0
        || key_id_len > crate::MAX_KEY_ID_BYTES
        || manifest_capacity > MAX_MANIFEST_BYTES
        || manifest_capacity == 0
    {
        return SDV_FOTA_INVALID_ARG;
    }
    // SAFETY: pointer lengths and lifetimes are part of the C function contract.
    let callbacks = unsafe { &*callbacks };
    if callbacks.source_read_exact.is_none()
        || callbacks.source_finish.is_none()
        || callbacks.read_state.is_none()
        || callbacks.begin_image.is_none()
        || callbacks.write_image.is_none()
        || callbacks.finish_image.is_none()
        || callbacks.abort_image.is_none()
        || callbacks.read_image.is_none()
        || callbacks.schedule_trial.is_none()
    {
        return SDV_FOTA_CALLBACK_ERROR;
    }
    let key_id = match core::str::from_utf8(unsafe { slice::from_raw_parts(key_id, key_id_len) }) {
        Ok(id) => id,
        Err(_) => return SDV_FOTA_INVALID_ARG,
    };
    let key_bytes: &[u8; 32] = unsafe { &*(public_key as *const [u8; 32]) };
    let ecu = match ecu_from_id(target_ecu) {
        Some(ecu) => ecu,
        None => return SDV_FOTA_INVALID_ARG,
    };
    let ports = Ports(callbacks);
    let state = match ports.state() {
        Ok(state) => state,
        Err(()) => return SDV_FOTA_CALLBACK_ERROR,
    };
    let context = ValidationContext {
        target_ecu: ecu,
        current_version: state.active_version.as_str(),
        highest_security_version: state.highest_security_version,
        config_schema,
        signal_if_version,
    };
    let trust_key = TrustKey {
        key_id,
        public_key: key_bytes,
    };
    let mut installer = FotaInstaller {
        storage: Ports(callbacks),
        boot_control: ports,
        trial_boots: 3,
    };
    // SAFETY: manifest buffer is writable and sized as declared by the caller.
    let manifest = unsafe { slice::from_raw_parts_mut(manifest_buffer, manifest_capacity) };
    match installer.stage_package_from_source(
        &mut Source(callbacks),
        manifest,
        &[trust_key],
        &context,
    ) {
        Ok(_) => SDV_FOTA_OK,
        Err(crate::StreamInstallError::Source(())) => SDV_FOTA_CALLBACK_ERROR,
        Err(crate::StreamInstallError::Storage(()))
        | Err(crate::StreamInstallError::BootControl(())) => SDV_FOTA_IO_ERROR,
        Err(crate::StreamInstallError::StorageSlotMismatch) => SDV_FOTA_IO_ERROR,
        Err(crate::StreamInstallError::TrialAlreadyPending) => SDV_FOTA_UPDATE_PENDING,
        Err(_) => SDV_FOTA_REJECTED,
    }
}

/// Resolve the persisted trial using the target's boot manager and health callbacks.
/// A healthy trial is committed. A failed or unavailable health check requests rollback.
#[no_mangle]
pub unsafe extern "C" fn sdv_fota_resolve_trial(callbacks: *const SdvFotaCallbacks) -> i32 {
    if callbacks.is_null() {
        return SDV_FOTA_INVALID_ARG;
    }
    // SAFETY: the caller guarantees the callbacks struct and context remain valid for the call.
    let callbacks = unsafe { &*callbacks };
    let mut boot = Ports(callbacks);
    let mut health = Health(callbacks);
    let trial_window_ms = match boot.state() {
        Ok(state) => match state.trial {
            Some(trial) => trial.trial_window_ms,
            None => return SDV_FOTA_REJECTED,
        },
        Err(()) => return SDV_FOTA_CALLBACK_ERROR,
    };
    match resolve_trial(&mut boot, &mut health, trial_window_ms) {
        Ok(crate::TrialResolution::Committed) => SDV_FOTA_OK,
        Ok(crate::TrialResolution::RolledBack) | Err(crate::TrialError::NoTrialPending) => {
            SDV_FOTA_REJECTED
        }
        Err(crate::TrialError::BootControl(())) | Err(crate::TrialError::HealthMonitor(())) => {
            SDV_FOTA_CALLBACK_ERROR
        }
    }
}

/// Ask the target boot manager to select the next slot and report whether it is a trial.
/// The target persists/consumes the boot attempt through its callback before returning.
#[no_mangle]
pub unsafe extern "C" fn sdv_fota_select_for_boot(
    callbacks: *const SdvFotaCallbacks,
    out_slot: *mut u8,
    out_is_trial: *mut u8,
) -> i32 {
    if callbacks.is_null() || out_slot.is_null() || out_is_trial.is_null() {
        return SDV_FOTA_INVALID_ARG;
    }
    // SAFETY: callbacks and output pointers are caller-owned and valid for this synchronous call.
    let mut ports = Ports(unsafe { &*callbacks });
    match ports.select_for_boot() {
        Ok(decision) => {
            unsafe {
                *out_slot = slot_id(decision.slot);
                *out_is_trial = u8::from(decision.trial);
            }
            SDV_FOTA_OK
        }
        Err(()) => SDV_FOTA_CALLBACK_ERROR,
    }
}
