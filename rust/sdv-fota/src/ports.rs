//! Service-level ports between FOTA policy and platform services.
//!
//! These traits are not AUTOSAR APIs. A Classic integration maps them to the selected
//! DCM/Csm/NvM/MemIf/Fls and bootloader services; the Linux adapter remains the Pi 5 host backend.

use crate::{BootDecision, BootState, CampaignCommand, Ecu, Slot, TrialImage};

/// Logical VCU-to-ECU campaign actions.
///
/// Implementations map these operations to the OEM's diagnostic client and transport stack
/// (for example DCM-facing application services over CAN or Ethernet). Calls submit an action;
/// ECU progress is reported separately through `CampaignManager::report`.
pub trait CampaignTransport {
    type Error;

    fn request_update(&mut self, ecu: Ecu) -> Result<(), Self::Error>;
    fn activate_trial(&mut self, ecu: Ecu) -> Result<(), Self::Error>;
    fn commit_trial(&mut self, ecu: Ecu) -> Result<(), Self::Error>;
    fn abort_update(&mut self, ecu: Ecu) -> Result<(), Self::Error>;
}

/// Submit an optional campaign action through an OEM transport port.
pub fn dispatch_campaign_command<T: CampaignTransport>(
    transport: &mut T,
    command: Option<CampaignCommand>,
) -> Result<(), T::Error> {
    let Some(command) = command else {
        return Ok(());
    };
    match command {
        CampaignCommand::Request(ecu) => transport.request_update(ecu),
        CampaignCommand::Activate(ecu) => transport.activate_trial(ecu),
        CampaignCommand::Commit(ecu) => transport.commit_trial(ecu),
        CampaignCommand::Abort(ecu) => transport.abort_update(ecu),
    }
}
/// Bounded-memory image partition port.
///
/// A Classic integration must implement this contract through the selected memory stack
/// (typically ECU abstraction / MemIf / Fls); this trait is not a replacement for those APIs.
pub trait ImageStorage {
    type Error;

    /// Prepare the slot opposite `active` for an image of `image_size` bytes.
    /// The active slot must remain bootable if this operation or a later write fails.
    fn begin_inactive_image(&mut self, active: Slot, image_size: u32) -> Result<Slot, Self::Error>;

    /// Write one sequential block. `offset` starts at zero and advances by each block length.
    fn write_image_chunk(
        &mut self,
        slot: Slot,
        offset: u32,
        chunk: &[u8],
    ) -> Result<(), Self::Error>;

    /// Finalize the inactive image after every expected byte has been written.
    fn finish_inactive_image(&mut self, slot: Slot) -> Result<(), Self::Error>;

    /// Discard an incomplete candidate after a failed transfer or write.
    fn abort_inactive_image(&mut self, slot: Slot);

    /// Read a block from a completed image. Returns zero only at end of image.
    fn read_image_chunk(
        &self,
        slot: Slot,
        offset: u32,
        buffer: &mut [u8],
    ) -> Result<usize, Self::Error>;
}

/// Transactional update-state and activation port.
///
/// A Classic adapter composes persistent metadata (typically NvM) with the OEM boot manager or
/// bootloader contract. It must preserve the trial/commit/rollback guarantees across reset.
pub trait BootControl {
    type Error;

    fn state(&self) -> Result<BootState, Self::Error>;
    fn schedule_trial(&mut self, trial: TrialImage) -> Result<(), Self::Error>;
    fn select_for_boot(&mut self) -> Result<BootDecision, Self::Error>;
    fn commit_trial(&mut self) -> Result<(), Self::Error>;
    fn rollback_trial(&mut self) -> Result<(), Self::Error>;
}

/// Checks whether a candidate process or ECU has met its platform health criteria.
///
/// This is an OEM health policy boundary; it is not a standardized AUTOSAR service by itself.
pub trait HealthMonitor {
    type Error;

    fn check(&mut self, slot: Slot, trial_window_ms: u32) -> Result<bool, Self::Error>;
}

/// Streams payload bytes after the signed manifest has been received and validated.
///
/// A DCM-backed implementation owns session/download framing. `read_exact` supplies only the
/// declared payload bytes, and `finish` confirms transport completion with no trailing bytes.
pub trait PackageSource {
    type Error;

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Self::Error>;
    fn finish(&mut self) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CampaignCommand;
    use std::vec::Vec;

    #[derive(Default)]
    struct Recorder(Vec<CampaignCommand>);

    impl CampaignTransport for Recorder {
        type Error = ();

        fn request_update(&mut self, ecu: Ecu) -> Result<(), Self::Error> {
            self.0.push(CampaignCommand::Request(ecu));
            Ok(())
        }

        fn activate_trial(&mut self, ecu: Ecu) -> Result<(), Self::Error> {
            self.0.push(CampaignCommand::Activate(ecu));
            Ok(())
        }

        fn commit_trial(&mut self, ecu: Ecu) -> Result<(), Self::Error> {
            self.0.push(CampaignCommand::Commit(ecu));
            Ok(())
        }

        fn abort_update(&mut self, ecu: Ecu) -> Result<(), Self::Error> {
            self.0.push(CampaignCommand::Abort(ecu));
            Ok(())
        }
    }

    #[test]
    fn dispatch_maps_each_campaign_action_to_the_transport_port() {
        let mut transport = Recorder::default();
        for command in [
            Some(CampaignCommand::Request(Ecu::Engine)),
            Some(CampaignCommand::Activate(Ecu::Brake)),
            Some(CampaignCommand::Commit(Ecu::Steering)),
            Some(CampaignCommand::Abort(Ecu::Engine)),
            None,
        ] {
            dispatch_campaign_command(&mut transport, command).unwrap();
        }
        assert_eq!(
            transport.0,
            [
                CampaignCommand::Request(Ecu::Engine),
                CampaignCommand::Activate(Ecu::Brake),
                CampaignCommand::Commit(Ecu::Steering),
                CampaignCommand::Abort(Ecu::Engine),
            ]
        );
    }
}
