/* SDV FOTA synchronous C bridge. This is an SDV integration ABI, not an AUTOSAR API. */
#ifndef SDV_FOTA_H
#define SDV_FOTA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SDV_FOTA_OK 0
#define SDV_FOTA_INVALID_ARG (-1)
#define SDV_FOTA_CALLBACK_ERROR 1
#define SDV_FOTA_REJECTED 2
#define SDV_FOTA_UPDATE_PENDING 3
#define SDV_FOTA_IO_ERROR 4

#define SDV_FOTA_SLOT_A 0
#define SDV_FOTA_SLOT_B 1
#define SDV_FOTA_ECU_ENGINE 0
#define SDV_FOTA_ECU_BRAKE 1
#define SDV_FOTA_ECU_STEERING 2
#define SDV_FOTA_VERSION_CAPACITY 32
#define SDV_FOTA_MAX_MANIFEST_BYTES (64u * 1024u)

typedef struct {
    uint8_t active_slot;
    uint8_t active_version[SDV_FOTA_VERSION_CAPACITY];
    size_t active_version_len;
    uint64_t highest_security_version;
    uint8_t trial_pending;
    uint8_t trial_slot;
    uint8_t trial_version[SDV_FOTA_VERSION_CAPACITY];
    size_t trial_version_len;
    uint64_t trial_security_version;
    uint32_t trial_boots_left;
    uint32_t trial_window_ms;
} sdv_fota_state_t;

typedef struct {
    uint8_t slot;
    uint8_t version[SDV_FOTA_VERSION_CAPACITY];
    size_t version_len;
    uint64_t security_version;
    uint32_t boots_left;
    uint32_t trial_window_ms;
} sdv_fota_trial_t;

/* Return zero on success. read_exact must fill the complete requested buffer. */
typedef struct {
    void *context;
    int32_t (*source_read_exact)(void *, uint8_t *, size_t);
    int32_t (*source_finish)(void *);
    int32_t (*read_state)(void *, sdv_fota_state_t *);
    int32_t (*begin_image)(void *, uint8_t active_slot, uint32_t image_size, uint8_t *out_slot);
    int32_t (*write_image)(void *, uint8_t slot, uint32_t offset, const uint8_t *, size_t);
    int32_t (*finish_image)(void *, uint8_t slot);
    void (*abort_image)(void *, uint8_t slot);
    int32_t (*read_image)(void *, uint8_t slot, uint32_t offset, uint8_t *, size_t capacity, size_t *out_read);
    int32_t (*schedule_trial)(void *, const sdv_fota_trial_t *);
    int32_t (*select_for_boot)(void *, uint8_t *out_slot, uint8_t *out_is_trial);
    int32_t (*commit_trial)(void *);
    int32_t (*rollback_trial)(void *);
    int32_t (*check_health)(void *, uint8_t slot, uint32_t trial_window_ms, uint8_t *out_healthy);
} sdv_fota_callbacks_t;

/*
 * Synchronously receive and stage one signed SDVF package. The caller owns every pointer and
 * callback context and must keep them valid until return. manifest_buffer must hold
 * manifest_capacity bytes (up to 64 KiB); public_key points to 32 bytes. ECU IDs are listed
 * above. Callback operations map to the selected DCM/Csm/NvM/MemIf/Fls and boot manager.
 * The active image must remain bootable if any callback fails. No pointers are retained.
 */
int32_t sdv_fota_stage_package(
    const sdv_fota_callbacks_t *callbacks,
    const uint8_t *key_id,
    size_t key_id_len,
    const uint8_t *public_key,
    uint8_t *manifest_buffer,
    size_t manifest_capacity,
    uint8_t target_ecu,
    uint32_t config_schema,
    uint32_t signal_if_version);

/* Run the persisted trial health decision: commit when healthy, otherwise request rollback. */
int32_t sdv_fota_resolve_trial(const sdv_fota_callbacks_t *callbacks);

#ifdef __cplusplus
}
#endif
#endif /* SDV_FOTA_H */
