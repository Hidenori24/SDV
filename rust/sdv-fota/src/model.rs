//! FOTA domain types shared by the package validator, update service, and platform ports.

use alloc::{string::String, vec::Vec};
use sdv_sec::ED25519_PUBLIC_KEY_LEN;
use serde::Deserialize;

/// Logical A/B software slot. Platform boot code maps these names to its own storage layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrialImage {
    pub slot: Slot,
    pub version: String,
    pub security_version: u64,
    pub boots_left: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootState {
    pub active: Slot,
    pub active_version: String,
    pub highest_security_version: u64,
    #[serde(default)]
    pub trial: Option<TrialImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootDecision {
    pub slot: Slot,
    pub trial: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecu {
    Engine,
    Brake,
    Steering,
}

impl Ecu {
    pub(crate) fn expected_swc(self) -> &'static str {
        match self {
            Self::Engine => "EngineSWC",
            Self::Brake => "BrakeSWC",
            Self::Steering => "SteeringSWC",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_schema: u32,
    pub image: Image,
    pub requires: Requirements,
    pub policy: UpdatePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub target_ecu: Ecu,
    pub swc: String,
    pub version: String,
    pub security_version: u64,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    pub signal_if_version: u32,
    pub config_schema: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePolicy {
    pub allow_downgrade: bool,
    pub trial_ms: u32,
}

/// The signed package envelope. `manifest_bytes` must be kept byte-for-byte as received.
pub struct Package<'a> {
    pub key_id: &'a str,
    pub signature: &'a [u8],
    pub manifest_bytes: &'a [u8],
    pub payload: &'a [u8],
}

pub struct TrustKey<'a> {
    pub key_id: &'a str,
    pub public_key: &'a [u8; ED25519_PUBLIC_KEY_LEN],
}

/// State and compatibility values supplied by the ECU that will receive the package.
pub struct ValidationContext<'a> {
    pub target_ecu: Ecu,
    pub current_version: &'a str,
    pub highest_security_version: u64,
    pub config_schema: u32,
    pub signal_if_version: u32,
}

pub struct ValidatedPackage<'a> {
    pub(crate) manifest: Manifest,
    pub(crate) payload: &'a [u8],
}

impl<'a> ValidatedPackage<'a> {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationError {
    InvalidPackageHeader,
    UnsupportedPackageFormat,
    PackageLengthMismatch,
    InvalidKeyId,
    UnknownKey,
    InvalidSignatureLength,
    ManifestTooLarge,
    InvalidSignature,
    InvalidManifest,
    UnsupportedManifestSchema,
    InvalidVersion,
    InvalidImageSize,
    InvalidSha256,
    InvalidPolicy,
    WrongTarget,
    IncompatibleSignalInterface,
    IncompatibleConfigSchema,
    Downgrade,
    SecurityVersionRollback,
    PayloadSizeMismatch,
    PayloadHashMismatch,
}
