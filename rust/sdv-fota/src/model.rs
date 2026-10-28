//! FOTA domain types shared by the package validator, update service, and platform ports.

use core::{fmt, ops::Deref};
use sdv_sec::ED25519_PUBLIC_KEY_LEN;
use serde::de::{Error as DeError, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const SWC_ID_CAPACITY: usize = 64;
const VERSION_CAPACITY: usize = 32;
const SHA256_HEX_CAPACITY: usize = 64;
pub const MAX_CONFIG_SCHEMA_VERSIONS: usize = 16;
pub const DEFAULT_TRIAL_WINDOW_MS: u32 = 30_000;
pub type VersionString = FixedString<VERSION_CAPACITY>;

fn default_trial_window_ms() -> u32 {
    DEFAULT_TRIAL_WINDOW_MS
}

/// UTF-8 string stored inline; deserialization rejects values larger than the chosen capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedString<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedString<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    pub fn try_from_str(value: &str) -> Option<Self> {
        Self::try_from_bytes(value.as_bytes())
    }

    pub(crate) fn try_from_bytes(value: &[u8]) -> Option<Self> {
        if value.len() > N {
            return None;
        }
        core::str::from_utf8(value).ok()?;
        let mut result = Self::new();
        result.bytes[..value.len()].copy_from_slice(value);
        result.len = value.len();
        Some(result)
    }
}

impl<const N: usize> Default for FixedString<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Deref for FixedString<N> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<const N: usize> fmt::Display for FixedString<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<const N: usize> PartialEq<&str> for FixedString<N> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<const N: usize> Serialize for FixedString<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de, const N: usize> Deserialize<'de> for FixedString<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FixedStringVisitor<const N: usize>;

        impl<const N: usize> Visitor<'_> for FixedStringVisitor<N> {
            type Value = FixedString<N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a UTF-8 string of at most {N} bytes")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                if value.len() > N {
                    return Err(E::invalid_length(value.len(), &self));
                }
                let mut result = FixedString::new();
                result.bytes[..value.len()].copy_from_slice(value.as_bytes());
                result.len = value.len();
                Ok(result)
            }
        }

        deserializer.deserialize_str(FixedStringVisitor::<N>)
    }
}

/// Bounded list used for the manifest's supported configuration schema versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedU32List<const N: usize> {
    values: [u32; N],
    len: usize,
}

impl<const N: usize> FixedU32List<N> {
    pub const fn new() -> Self {
        Self {
            values: [0; N],
            len: 0,
        }
    }

    pub(crate) fn try_push(&mut self, value: u32) -> bool {
        if self.len == N {
            return false;
        }
        self.values[self.len] = value;
        self.len += 1;
        true
    }
}

impl<const N: usize> Default for FixedU32List<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Deref for FixedU32List<N> {
    type Target = [u32];

    fn deref(&self) -> &Self::Target {
        &self.values[..self.len]
    }
}

impl<'de, const N: usize> Deserialize<'de> for FixedU32List<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FixedU32ListVisitor<const N: usize>;

        impl<'de, const N: usize> Visitor<'de> for FixedU32ListVisitor<N> {
            type Value = FixedU32List<N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a list of at most {N} unsigned 32-bit integers")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut result = FixedU32List::new();
                while let Some(value) = sequence.next_element()? {
                    if result.len == N {
                        return Err(A::Error::invalid_length(N + 1, &self));
                    }
                    result.values[result.len] = value;
                    result.len += 1;
                }
                Ok(result)
            }
        }

        deserializer.deserialize_seq(FixedU32ListVisitor::<N>)
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrialImage {
    pub slot: Slot,
    pub version: VersionString,
    pub security_version: u64,
    pub boots_left: u32,
    #[serde(default = "default_trial_window_ms")]
    pub trial_window_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootState {
    pub active: Slot,
    pub active_version: VersionString,
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
    /// The complete vehicle-control application hosted on the Raspberry Pi.
    Vehicle,
}

impl Ecu {
    pub(crate) fn expected_swc(self) -> &'static str {
        match self {
            Self::Engine => "EngineSWC",
            Self::Brake => "BrakeSWC",
            Self::Steering => "SteeringSWC",
            Self::Vehicle => "VehicleControlApp",
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
    pub swc: FixedString<SWC_ID_CAPACITY>,
    pub version: FixedString<VERSION_CAPACITY>,
    pub security_version: u64,
    pub size_bytes: u64,
    pub sha256: FixedString<SHA256_HEX_CAPACITY>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    pub signal_if_version: u32,
    pub config_schema: FixedU32List<MAX_CONFIG_SCHEMA_VERSIONS>,
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
