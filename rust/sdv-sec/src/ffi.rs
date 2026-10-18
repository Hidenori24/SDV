//! C ABI (`include/sdv_sec.h`). Every function takes caller-owned buffers and returns an `int` status:
//! 0 ok, positive = a specific failure (below), `-1` = a null pointer or an invalid length was passed.
//! No function allocates, panics on valid input, or keeps state.
use crate::*;
use core::slice;

pub const SDV_SEC_OK: i32 = 0;
pub const SDV_SEC_BAD_KEY: i32 = 1;
pub const SDV_SEC_BAD_SIGNATURE: i32 = 2;
pub const SDV_SEC_BAD_TAG: i32 = 3;
pub const SDV_SEC_STALE: i32 = 4;
pub const SDV_SEC_INVALID_ARG: i32 = -1;

/// A readable buffer of `len` bytes (`len == 0` accepts a null pointer).
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        Some(&[])
    } else if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees `ptr` points to `len` readable bytes.
        Some(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

/// A fixed-size array view of a caller buffer.
unsafe fn array<'a, const N: usize>(ptr: *const u8) -> Option<&'a [u8; N]> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees `ptr` points to `N` readable bytes.
        Some(unsafe { &*(ptr as *const [u8; N]) })
    }
}

unsafe fn output<'a, const N: usize>(ptr: *mut u8) -> Option<&'a mut [u8; N]> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees `ptr` points to `N` writable bytes.
        Some(unsafe { &mut *(ptr as *mut [u8; N]) })
    }
}

/// # Safety
/// `data` must point to `len` readable bytes (or be null with `len == 0`); `out` to 32 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_sha256(data: *const u8, len: usize, out: *mut u8) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(d), Some(o)) = (unsafe { input(data, len) }, unsafe {
        output::<SHA256_LEN>(out)
    }) else {
        return SDV_SEC_INVALID_ARG;
    };
    *o = sha256(d);
    SDV_SEC_OK
}

/// # Safety
/// `key` / `msg` must point to `key_len` / `msg_len` readable bytes (or be null with length 0); `out` to 32 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_hmac_sha256(
    key: *const u8,
    key_len: usize,
    msg: *const u8,
    msg_len: usize,
    out: *mut u8,
) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(k), Some(m), Some(o)) = (
        unsafe { input(key, key_len) },
        unsafe { input(msg, msg_len) },
        unsafe { output::<SHA256_LEN>(out) },
    ) else {
        return SDV_SEC_INVALID_ARG;
    };
    *o = hmac_sha256(k, m);
    SDV_SEC_OK
}

/// Constant-time comparison of two `len`-byte buffers. Returns 1 when equal, 0 when not, -1 for a null pointer.
///
/// # Safety
/// `a` and `b` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_ct_eq(a: *const u8, b: *const u8, len: usize) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(x), Some(y)) = (unsafe { input(a, len) }, unsafe { input(b, len) }) else {
        return SDV_SEC_INVALID_ARG;
    };
    ct_eq(x, y) as i32
}

/// Verify an Ed25519 signature. Returns 0 ok, 1 bad key, 2 bad signature.
///
/// # Safety
/// `public_key` must point to 32 readable bytes, `msg` to `msg_len`, `signature` to 64.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_ed25519_verify(
    public_key: *const u8,
    msg: *const u8,
    msg_len: usize,
    signature: *const u8,
) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(pk), Some(m), Some(sig)) = (
        unsafe { array::<ED25519_PUBLIC_KEY_LEN>(public_key) },
        unsafe { input(msg, msg_len) },
        unsafe { array::<ED25519_SIGNATURE_LEN>(signature) },
    ) else {
        return SDV_SEC_INVALID_ARG;
    };
    match ed25519_verify(pk, m, sig) {
        Ok(()) => SDV_SEC_OK,
        Err(VerifyError::BadKey) => SDV_SEC_BAD_KEY,
        Err(VerifyError::BadSignature) => SDV_SEC_BAD_SIGNATURE,
    }
}

/// Compute the 8-byte frame MAC.
///
/// # Safety
/// `key` / `payload` must point to `key_len` / `payload_len` readable bytes (or be null with length 0); `out` to 8 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_frame_mac(
    key: *const u8,
    key_len: usize,
    data_id: u16,
    freshness: u32,
    payload: *const u8,
    payload_len: usize,
    out: *mut u8,
) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(k), Some(p), Some(o)) = (
        unsafe { input(key, key_len) },
        unsafe { input(payload, payload_len) },
        unsafe { output::<FRAME_MAC_LEN>(out) },
    ) else {
        return SDV_SEC_INVALID_ARG;
    };
    *o = frame_mac(k, data_id, freshness, p);
    SDV_SEC_OK
}

/// Verify an 8-byte frame MAC. Returns 0 ok, 3 bad tag.
///
/// # Safety
/// `key` / `payload` must point to `key_len` / `payload_len` readable bytes (or be null with length 0); `tag` to 8.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_frame_mac_verify(
    key: *const u8,
    key_len: usize,
    data_id: u16,
    freshness: u32,
    payload: *const u8,
    payload_len: usize,
    tag: *const u8,
) -> i32 {
    // SAFETY: forwarded from the caller's contract.
    let (Some(k), Some(p), Some(t)) = (
        unsafe { input(key, key_len) },
        unsafe { input(payload, payload_len) },
        unsafe { array::<FRAME_MAC_LEN>(tag) },
    ) else {
        return SDV_SEC_INVALID_ARG;
    };
    match frame_mac_verify(k, data_id, freshness, p, t) {
        Ok(()) => SDV_SEC_OK,
        Err(MacError::BadTag) => SDV_SEC_BAD_TAG,
    }
}

/// Replay check: accept `counter` only if it is greater than `*last` (and then store it). Returns 0 accepted, 4 stale.
///
/// # Safety
/// `last` must point to a valid, writable `u32` owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn sdv_sec_freshness_accept(last: *mut u32, counter: u32) -> i32 {
    if last.is_null() {
        return SDV_SEC_INVALID_ARG;
    }
    // SAFETY: non-null and valid by the caller's contract.
    let last = unsafe { &mut *last };
    let mut f = Freshness::new(*last);
    if f.accept(counter) {
        *last = f.last();
        SDV_SEC_OK
    } else {
        SDV_SEC_STALE
    }
}
