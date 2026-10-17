/* sdv-sec C ABI (ADR-0005). Hand-written; tests/header.rs checks it against the exported symbols.
 *
 * All buffers are owned by the caller; nothing allocates or keeps state.
 * Return value: 0 ok, positive = a specific failure, -1 = null pointer / invalid argument.
 */
#ifndef SDV_SEC_H
#define SDV_SEC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SDV_SEC_OK 0
#define SDV_SEC_BAD_KEY 1
#define SDV_SEC_BAD_SIGNATURE 2
#define SDV_SEC_BAD_TAG 3
#define SDV_SEC_STALE 4
#define SDV_SEC_INVALID_ARG (-1)

#define SDV_SEC_SHA256_LEN 32
#define SDV_SEC_ED25519_PUBLIC_KEY_LEN 32
#define SDV_SEC_ED25519_SIGNATURE_LEN 64
#define SDV_SEC_FRAME_MAC_LEN 8

/* out: 32 bytes */
int32_t sdv_sec_sha256(const uint8_t* data, size_t len, uint8_t* out);

/* out: 32 bytes. Any key length. */
int32_t sdv_sec_hmac_sha256(const uint8_t* key, size_t key_len, const uint8_t* msg, size_t msg_len, uint8_t* out);

/* Constant-time compare of two len-byte buffers: 1 equal, 0 different, -1 null pointer. */
int32_t sdv_sec_ct_eq(const uint8_t* a, const uint8_t* b, size_t len);

/* public_key: 32 bytes, signature: 64 bytes. 0 ok, 1 bad key, 2 bad signature. Verify before you parse msg. */
int32_t sdv_sec_ed25519_verify(const uint8_t* public_key, const uint8_t* msg, size_t msg_len, const uint8_t* signature);

/* out: 8 bytes = first 8 bytes of HMAC-SHA256(key, data_id(u16 BE) || freshness(u32 BE) || payload). */
int32_t sdv_sec_frame_mac(const uint8_t* key, size_t key_len, uint16_t data_id, uint32_t freshness,
                          const uint8_t* payload, size_t payload_len, uint8_t* out);

/* tag: 8 bytes. 0 ok, 3 bad tag. Constant time. Check freshness only after this succeeds. */
int32_t sdv_sec_frame_mac_verify(const uint8_t* key, size_t key_len, uint16_t data_id, uint32_t freshness,
                                 const uint8_t* payload, size_t payload_len, const uint8_t* tag);

/* Accept counter only if it is greater than *last (and then store it). 0 accepted, 4 stale. */
int32_t sdv_sec_freshness_accept(uint32_t* last, uint32_t counter);

#ifdef __cplusplus
}
#endif

#endif /* SDV_SEC_H */
