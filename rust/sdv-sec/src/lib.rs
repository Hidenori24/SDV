//! SDV communication-security foundation (ADR-0005).
//!
//! What it provides, and nothing else:
//! - [`sha256`] — integrity (corruption detection)
//! - [`ed25519_verify`] — origin / tamper detection of a signed blob (an update package). The ECU holds only public keys.
//! - [`ed25519_sign`] / [`ed25519_public_key`] (feature `sign`, host only): used by the signing tool and the tests
//! - [`frame_mac`] / [`frame_mac_verify`] + [`Freshness`]: SecOC-style frame authentication
//!   (data id + freshness counter + payload, truncated HMAC-SHA256)
//!
//! Rules (they make the same code usable on the car): `no_std`, no heap, no global state, no clock or file access.
//! Everything works on buffers the caller passes. The cryptography is the RustCrypto implementation; nothing is
//! hand-rolled. The C ABI is in [`ffi`] and `include/sdv_sec.h`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

use ed25519_dalek::{Signature, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub mod ffi;

pub const SHA256_LEN: usize = 32;
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
pub const ED25519_SIGNATURE_LEN: usize = 64;
/// Length of the truncated frame MAC (64 bit, as commonly used for CAN-class links).
pub const FRAME_MAC_LEN: usize = 8;

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; SHA256_LEN] {
    Sha256::digest(data).into()
}

/// HMAC-SHA256. Any key length is accepted (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; SHA256_LEN] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

/// Constant-time equality (false when the lengths differ).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The public key is not a valid (or is a weak) Ed25519 key.
    BadKey,
    /// The signature does not match the message and key.
    BadSignature,
}

/// Verify an Ed25519 signature (strict: rejects weak keys and malleable signatures).
/// Verify first, then parse: do not interpret `msg` before this returns `Ok`.
pub fn ed25519_verify(
    public_key: &[u8; ED25519_PUBLIC_KEY_LEN],
    msg: &[u8],
    signature: &[u8; ED25519_SIGNATURE_LEN],
) -> Result<(), VerifyError> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| VerifyError::BadKey)?;
    if key.is_weak() {
        return Err(VerifyError::BadKey);
    }
    key.verify_strict(msg, &Signature::from_bytes(signature))
        .map_err(|_| VerifyError::BadSignature)
}

/// Public key for a 32-byte Ed25519 seed (host only).
#[cfg(feature = "sign")]
pub fn ed25519_public_key(seed: &[u8; 32]) -> [u8; ED25519_PUBLIC_KEY_LEN] {
    ed25519_dalek::SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes()
}

/// Sign `msg` with a 32-byte Ed25519 seed (host only: the signing tool, tests, the demo key).
#[cfg(feature = "sign")]
pub fn ed25519_sign(seed: &[u8; 32], msg: &[u8]) -> [u8; ED25519_SIGNATURE_LEN] {
    use ed25519_dalek::Signer;
    ed25519_dalek::SigningKey::from_bytes(seed)
        .sign(msg)
        .to_bytes()
}

/// Frame MAC: the first [`FRAME_MAC_LEN`] bytes of
/// `HMAC-SHA256(key, data_id (u16 big endian) || freshness (u32 big endian) || payload)`.
///
/// `data_id` ties the tag to one frame type (a tag for one frame is no good for another) and `freshness` to one
/// transmission (see [`Freshness`]).
pub fn frame_mac(key: &[u8], data_id: u16, freshness: u32, payload: &[u8]) -> [u8; FRAME_MAC_LEN] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&data_id.to_be_bytes());
    mac.update(&freshness.to_be_bytes());
    mac.update(payload);
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; FRAME_MAC_LEN];
    tag.copy_from_slice(&full[..FRAME_MAC_LEN]);
    tag
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacError {
    /// The tag does not match (wrong key, tampered data, wrong frame type or counter).
    BadTag,
}

/// Check a received frame MAC in constant time. This does not check freshness: call [`Freshness::accept`]
/// **after** this succeeds (an unauthenticated frame must not move the counter).
pub fn frame_mac_verify(
    key: &[u8],
    data_id: u16,
    freshness: u32,
    payload: &[u8],
    tag: &[u8; FRAME_MAC_LEN],
) -> Result<(), MacError> {
    if ct_eq(&frame_mac(key, data_id, freshness, payload), tag) {
        Ok(())
    } else {
        Err(MacError::BadTag)
    }
}

/// Receiver-side replay protection: a frame is fresh only if its counter is strictly greater than the last
/// accepted one. The counter is a `u32`; before it can wrap, the key must be replaced (re-keying is out of scope).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Freshness {
    last: u32,
}

impl Freshness {
    pub const fn new(last_accepted: u32) -> Self {
        Self {
            last: last_accepted,
        }
    }

    pub fn last(&self) -> u32 {
        self.last
    }

    /// Accept `counter` if it is newer than the last accepted one, and remember it.
    pub fn accept(&mut self, counter: u32) -> bool {
        if counter > self.last {
            self.last = counter;
            true
        } else {
            false
        }
    }
}
