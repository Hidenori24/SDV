//! Signed package framing, manifest validation, and compatibility policy.

use crate::model::{
    Manifest, Package, TrustKey, ValidatedPackage, ValidationContext, ValidationError,
};
use alloc::vec::Vec;
use sdv_sec::{ed25519_verify, sha256, ED25519_SIGNATURE_LEN};

pub const MANIFEST_SCHEMA: u32 = 1;
pub const SIGNAL_IF_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_KEY_ID_BYTES: usize = 64;
const PACKAGE_MAGIC: &[u8; 4] = b"SDVF";
const PACKAGE_FORMAT_VERSION: u8 = 1;
const PACKAGE_HEADER_BYTES: usize = 18;
const PACKAGE_PREFIX_BYTES: usize = PACKAGE_HEADER_BYTES + ED25519_SIGNATURE_LEN;

/// Parse a v1 binary envelope without interpreting the manifest or payload.
///
/// Layout: `SDVF` magic, format version (u8), key-ID length (u8), manifest length (u32 BE),
/// payload length (u64 BE), 64-byte Ed25519 signature, UTF-8 key ID, exact manifest bytes,
/// payload bytes. Trailing bytes are rejected. The signature covers only the manifest bytes.
pub fn parse_package(bytes: &[u8]) -> Result<Package<'_>, ValidationError> {
    if bytes.len() < PACKAGE_PREFIX_BYTES || &bytes[..4] != PACKAGE_MAGIC {
        return Err(ValidationError::InvalidPackageHeader);
    }
    if bytes[4] != PACKAGE_FORMAT_VERSION {
        return Err(ValidationError::UnsupportedPackageFormat);
    }
    let key_id_len = bytes[5] as usize;
    let manifest_len = u32::from_be_bytes(bytes[6..10].try_into().unwrap()) as usize;
    let payload_len_u64 = u64::from_be_bytes(bytes[10..18].try_into().unwrap());
    let payload_len =
        usize::try_from(payload_len_u64).map_err(|_| ValidationError::PackageLengthMismatch)?;
    if key_id_len == 0 || key_id_len > MAX_KEY_ID_BYTES {
        return Err(ValidationError::InvalidKeyId);
    }
    if manifest_len > MAX_MANIFEST_BYTES {
        return Err(ValidationError::ManifestTooLarge);
    }
    let total_len = PACKAGE_PREFIX_BYTES
        .checked_add(key_id_len)
        .and_then(|n| n.checked_add(manifest_len))
        .and_then(|n| n.checked_add(payload_len))
        .ok_or(ValidationError::PackageLengthMismatch)?;
    if total_len != bytes.len() {
        return Err(ValidationError::PackageLengthMismatch);
    }
    let key_start = PACKAGE_PREFIX_BYTES;
    let signature_start = PACKAGE_HEADER_BYTES;
    let manifest_start = key_start + key_id_len;
    let payload_start = manifest_start + manifest_len;
    let key_id = core::str::from_utf8(&bytes[key_start..manifest_start])
        .map_err(|_| ValidationError::InvalidKeyId)?;
    Ok(Package {
        key_id,
        signature: &bytes[signature_start..PACKAGE_PREFIX_BYTES],
        manifest_bytes: &bytes[manifest_start..payload_start],
        payload: &bytes[payload_start..],
    })
}

/// Encode the v1 envelope. The caller supplies the signature over the exact manifest bytes.
pub fn encode_package(
    key_id: &str,
    signature: &[u8; ED25519_SIGNATURE_LEN],
    manifest_bytes: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>, ValidationError> {
    validate_key_id(key_id)?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ValidationError::ManifestTooLarge);
    }
    let key_id_len = u8::try_from(key_id.len()).map_err(|_| ValidationError::InvalidKeyId)?;
    let manifest_len =
        u32::try_from(manifest_bytes.len()).map_err(|_| ValidationError::ManifestTooLarge)?;
    let payload_len =
        u64::try_from(payload.len()).map_err(|_| ValidationError::PackageLengthMismatch)?;
    let capacity = PACKAGE_PREFIX_BYTES
        .checked_add(key_id.len())
        .and_then(|n| n.checked_add(manifest_bytes.len()))
        .and_then(|n| n.checked_add(payload.len()))
        .ok_or(ValidationError::PackageLengthMismatch)?;
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(PACKAGE_MAGIC);
    bytes.push(PACKAGE_FORMAT_VERSION);
    bytes.push(key_id_len);
    bytes.extend_from_slice(&manifest_len.to_be_bytes());
    bytes.extend_from_slice(&payload_len.to_be_bytes());
    bytes.extend_from_slice(signature);
    bytes.extend_from_slice(key_id.as_bytes());
    bytes.extend_from_slice(manifest_bytes);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// Verify the raw manifest signature first, then parse and validate the manifest and payload.
///
/// The package is accepted only for the expected ECU, supported signal interface and active
/// Config schema. This is validation only; it does not write to NVM or activate software.
pub fn validate_package<'a>(
    package: &Package<'a>,
    trust_keys: &[TrustKey<'_>],
    context: &ValidationContext<'_>,
) -> Result<ValidatedPackage<'a>, ValidationError> {
    validate_key_id(package.key_id)?;
    let key = trust_keys
        .iter()
        .find(|key| key.key_id == package.key_id)
        .ok_or(ValidationError::UnknownKey)?;
    if package.signature.len() != ED25519_SIGNATURE_LEN {
        return Err(ValidationError::InvalidSignatureLength);
    }
    if package.manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ValidationError::ManifestTooLarge);
    }
    let signature: &[u8; ED25519_SIGNATURE_LEN] = package
        .signature
        .try_into()
        .map_err(|_| ValidationError::InvalidSignatureLength)?;
    ed25519_verify(key.public_key, package.manifest_bytes, signature)
        .map_err(|_| ValidationError::InvalidSignature)?;

    let manifest: Manifest = serde_json::from_slice(package.manifest_bytes)
        .map_err(|_| ValidationError::InvalidManifest)?;
    validate_manifest(&manifest, package.payload, context)?;
    Ok(ValidatedPackage {
        manifest,
        payload: package.payload,
    })
}

fn validate_key_id(key_id: &str) -> Result<(), ValidationError> {
    if key_id.is_empty()
        || key_id.len() > MAX_KEY_ID_BYTES
        || !key_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ValidationError::InvalidKeyId);
    }
    Ok(())
}

fn validate_manifest(
    manifest: &Manifest,
    payload: &[u8],
    context: &ValidationContext<'_>,
) -> Result<(), ValidationError> {
    if manifest.manifest_schema != MANIFEST_SCHEMA {
        return Err(ValidationError::UnsupportedManifestSchema);
    }
    if manifest.image.swc != manifest.image.target_ecu.expected_swc()
        || manifest.image.target_ecu != context.target_ecu
    {
        return Err(ValidationError::WrongTarget);
    }
    let incoming = parse_version(&manifest.image.version).ok_or(ValidationError::InvalidVersion)?;
    let current = parse_version(context.current_version).ok_or(ValidationError::InvalidVersion)?;
    if !manifest.policy.allow_downgrade && incoming <= current {
        return Err(ValidationError::Downgrade);
    }
    if manifest.image.security_version < context.highest_security_version {
        return Err(ValidationError::SecurityVersionRollback);
    }
    if manifest.image.sha256.len() != 64
        || !manifest.image.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ValidationError::InvalidSha256);
    }
    if manifest.image.size_bytes == 0 {
        return Err(ValidationError::InvalidImageSize);
    }
    if manifest.policy.trial_ms == 0 {
        return Err(ValidationError::InvalidPolicy);
    }
    if manifest.requires.signal_if_version != context.signal_if_version {
        return Err(ValidationError::IncompatibleSignalInterface);
    }
    if manifest.requires.config_schema.is_empty()
        || manifest
            .requires
            .config_schema
            .iter()
            .any(|schema| *schema == 0)
        || manifest
            .requires
            .config_schema
            .iter()
            .enumerate()
            .any(|(i, schema)| manifest.requires.config_schema[..i].contains(schema))
        || !manifest
            .requires
            .config_schema
            .contains(&context.config_schema)
    {
        return Err(ValidationError::IncompatibleConfigSchema);
    }
    let payload_len =
        u64::try_from(payload.len()).map_err(|_| ValidationError::PayloadSizeMismatch)?;
    if manifest.image.size_bytes != payload_len {
        return Err(ValidationError::PayloadSizeMismatch);
    }
    let expected = decode_sha256(&manifest.image.sha256).ok_or(ValidationError::InvalidSha256)?;
    if sha256(payload) != expected {
        return Err(ValidationError::PayloadHashMismatch);
    }
    Ok(())
}

pub(crate) fn decode_sha256(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        digest[i] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(digest)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parse_version_part(parts.next()?)?;
    let minor = parse_version_part(parts.next()?)?;
    let patch = parse_version_part(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_version_part(part: &str) -> Option<u32> {
    if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
        return None;
    }
    part.parse().ok()
}
