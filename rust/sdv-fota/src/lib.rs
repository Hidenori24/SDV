//! FOTA service, signed package model, service-level platform ports, and host adapters.
//!
//! The Pi 5 Linux adapter remains available for host execution. AUTOSAR Classic integration is
//! supplied by a target-specific binding to the selected BSW/MCAL and bootloader APIs.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod model;
mod package;
mod ports;
mod service;

pub use model::{
    BootDecision, BootState, Ecu, Image, Manifest, Package, Requirements, Slot, TrialImage,
    TrustKey, UpdatePolicy, ValidatedPackage, ValidationContext, ValidationError,
};
pub use package::{
    encode_package, parse_package, validate_package, validate_signed_manifest, MANIFEST_SCHEMA,
    MAX_KEY_ID_BYTES, MAX_MANIFEST_BYTES, SIGNAL_IF_VERSION,
};
pub use ports::{BootControl, HealthMonitor, ImageStorage, PackageSource};
pub use service::{
    resolve_trial, FotaInstaller, InstallError, StreamInstallError, TrialError, TrialResolution,
};

#[cfg(all(feature = "std", target_os = "linux"))]
pub mod linux;
