#include "sdv_fota.h"

_Static_assert(sizeof(sdv_fota_campaign_t) == 7, "campaign state ABI size changed");
_Static_assert(SDV_FOTA_CAMPAIGN_CHECKPOINT_SIZE == 16, "campaign checkpoint size changed");

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

    sdv_fota_campaign_t campaign;
    sdv_fota_campaign_command_t command = {0};
    sdv_fota_vehicle_conditions_t conditions = {
        .stopped = 1,
        .safety_state = SDV_FOTA_SAFETY_NORMAL,
        .other_update_active = 0,
    };
    const uint8_t targets[] = {SDV_FOTA_ECU_ENGINE, SDV_FOTA_ECU_BRAKE};
    if (sdv_fota_campaign_init(NULL) != SDV_FOTA_INVALID_ARG ||
        sdv_fota_campaign_init(&campaign) != SDV_FOTA_OK) {
        return 6;
    }
    if (sdv_fota_campaign_start(&campaign, targets, 2, &conditions, &command) != SDV_FOTA_OK ||
        command.command != SDV_FOTA_CAMPAIGN_REQUEST || command.ecu != SDV_FOTA_ECU_ENGINE) {
        return 7;
    }
    if (sdv_fota_campaign_report(&campaign, SDV_FOTA_ECU_ENGINE,
                                SDV_FOTA_ECU_STATUS_STAGED, &conditions, &command) != SDV_FOTA_OK ||
        command.command != SDV_FOTA_CAMPAIGN_ACTIVATE || command.ecu != SDV_FOTA_ECU_ENGINE) {
        return 8;
    }

    uint8_t checkpoint[SDV_FOTA_CAMPAIGN_CHECKPOINT_SIZE];
    if (sdv_fota_campaign_checkpoint(&campaign, checkpoint, sizeof(checkpoint)) != SDV_FOTA_OK ||
        sdv_fota_campaign_checkpoint(&campaign, checkpoint, sizeof(checkpoint) - 1) !=
            SDV_FOTA_INVALID_ARG) {
        return 9;
    }
    sdv_fota_campaign_t resumed_campaign;
    uint8_t corrupt_checkpoint[SDV_FOTA_CAMPAIGN_CHECKPOINT_SIZE];
    for (size_t i = 0; i < sizeof(checkpoint); ++i) {
        corrupt_checkpoint[i] = checkpoint[i];
    }
    corrupt_checkpoint[0] ^= 1;
    if (sdv_fota_campaign_restore(&resumed_campaign, corrupt_checkpoint,
                                  sizeof(corrupt_checkpoint)) != SDV_FOTA_REJECTED ||
        sdv_fota_campaign_restore(&resumed_campaign, checkpoint, sizeof(checkpoint)) != SDV_FOTA_OK) {
        return 10;
    }
    if (sdv_fota_campaign_report(&resumed_campaign, SDV_FOTA_ECU_ENGINE,
                                SDV_FOTA_ECU_STATUS_TRIAL_HEALTHY, &conditions, &command) != SDV_FOTA_OK ||
        command.command != SDV_FOTA_CAMPAIGN_COMMIT) {
        return 11;
    }
    if (sdv_fota_campaign_report(&resumed_campaign, SDV_FOTA_ECU_ENGINE,
                                SDV_FOTA_ECU_STATUS_COMMITTED, &conditions, &command) != SDV_FOTA_OK ||
        command.command != SDV_FOTA_CAMPAIGN_REQUEST || command.ecu != SDV_FOTA_ECU_BRAKE) {
        return 12;
    }
    conditions.stopped = 0;
    if (sdv_fota_campaign_recheck(&resumed_campaign, &conditions, &command) != SDV_FOTA_OK ||
        command.command != SDV_FOTA_CAMPAIGN_ABORT || command.ecu != SDV_FOTA_ECU_BRAKE) {
        return 13;
    }
    uint8_t campaign_state = 0xff;
    uint8_t campaign_ecu = 0xff;
    if (sdv_fota_campaign_state(&resumed_campaign, &campaign_state, &campaign_ecu) != SDV_FOTA_OK ||
        campaign_state != SDV_FOTA_CAMPAIGN_STATE_FAILED ||
        campaign_ecu != SDV_FOTA_ECU_BRAKE) {
        return 14;
    }
    return 0;
}
