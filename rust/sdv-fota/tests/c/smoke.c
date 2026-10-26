#include "sdv_fota.h"

int main(void) {
    /* Null arguments must fail before any callback or memory access. */
    if (sdv_fota_stage_package(NULL, NULL, 0, NULL, NULL, 0,
                               SDV_FOTA_ECU_ENGINE, 1, 1) != SDV_FOTA_INVALID_ARG) {
        return 1;
    }
    sdv_fota_callbacks_t callbacks = {0};
    uint8_t key_id = 'k';
    uint8_t key[32] = {0};
    uint8_t manifest[1] = {0};
    if (sdv_fota_stage_package(&callbacks, &key_id, 1, key, manifest, sizeof(manifest),
                               SDV_FOTA_ECU_ENGINE, 1, 1) != SDV_FOTA_CALLBACK_ERROR) {
        return 2;
    }
    if (sdv_fota_resolve_trial(&callbacks) != SDV_FOTA_CALLBACK_ERROR) {
        return 3;
    }
    uint8_t slot = 0xff;
    uint8_t is_trial = 0xff;
    if (sdv_fota_select_for_boot(&callbacks, &slot, &is_trial) != SDV_FOTA_CALLBACK_ERROR) {
        return 4;
    }
    if (sdv_fota_select_for_boot(NULL, &slot, &is_trial) != SDV_FOTA_INVALID_ARG) {
        return 5;
    }
    return 0;
}
