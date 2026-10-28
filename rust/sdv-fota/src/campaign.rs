//! VCU-side policy for running a multi-ECU update campaign sequentially.
//!
//! This allocator-free state machine emits commands for a diagnostic or Classic BSW adapter to
//! transport. It does not implement a vehicle communication protocol.

use crate::Ecu;

pub const MAX_CAMPAIGN_ECUS: usize = 3;
pub const CAMPAIGN_CHECKPOINT_SIZE: usize = 16;

const CHECKPOINT_MAGIC: [u8; 4] = *b"SDVC";
const CHECKPOINT_VERSION: u8 = 1;
const CHECKPOINT_MAGIC_OFFSET: usize = 0;
const CHECKPOINT_VERSION_OFFSET: usize = 4;
const CHECKPOINT_TARGETS_OFFSET: usize = 5;
const CHECKPOINT_TARGET_COUNT_OFFSET: usize = 8;
const CHECKPOINT_TARGET_INDEX_OFFSET: usize = 9;
const CHECKPOINT_PHASE_OFFSET: usize = 10;
const CHECKPOINT_STATE_OFFSET: usize = 11;
const CHECKPOINT_CRC_OFFSET: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleSafety {
    Normal,
    Degraded,
    EStop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VehicleConditions {
    pub stopped: bool,
    pub safety: VehicleSafety,
    /// True when an update outside this manager is already active on another ECU.
    pub other_update_active: bool,
}

impl VehicleConditions {
    fn permits_update(self) -> bool {
        self.stopped && self.safety == VehicleSafety::Normal && !self.other_update_active
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CampaignPlan {
    targets: [Option<Ecu>; MAX_CAMPAIGN_ECUS],
    len: u8,
}

impl CampaignPlan {
    pub fn new(targets: &[Ecu]) -> Result<Self, CampaignError> {
        if targets.is_empty() {
            return Err(CampaignError::EmptyPlan);
        }
        if targets.len() > MAX_CAMPAIGN_ECUS {
            return Err(CampaignError::TooManyTargets);
        }
        let mut result = Self {
            targets: [None; MAX_CAMPAIGN_ECUS],
            len: targets.len() as u8,
        };
        for (index, target) in targets.iter().copied().enumerate() {
            if targets[..index].contains(&target) {
                return Err(CampaignError::DuplicateTarget);
            }
            result.targets[index] = Some(target);
        }
        Ok(result)
    }

    fn target(&self, index: u8) -> Option<Ecu> {
        (index < self.len)
            .then(|| self.targets[index as usize])
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EcuUpdateStatus {
    Staged,
    TrialHealthy,
    Committed,
    RolledBack,
    Rejected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CampaignCommand {
    Request(Ecu),
    Activate(Ecu),
    Commit(Ecu),
    Abort(Ecu),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CampaignState {
    Idle,
    Updating { ecu: Ecu },
    Completed,
    Failed { ecu: Ecu },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingStage,
    AwaitingTrialHealth,
    AwaitingCommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CampaignError {
    EmptyPlan,
    TooManyTargets,
    DuplicateTarget,
    VehicleMoving,
    SafetyNotNormal,
    AnotherUpdateActive,
    CampaignInProgress,
    NoCampaign,
    UnexpectedEcu,
    UnexpectedStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CampaignCheckpointError {
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion,
    ChecksumMismatch,
    InvalidState,
}

/// Serial campaign controller. It never has more than one ECU request outstanding.
///
/// This plain-byte layout is also exposed in `sdv_fota.h` so Classic C integration can allocate
/// the state statically and pass it to the campaign C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CampaignManager {
    targets: [u8; MAX_CAMPAIGN_ECUS],
    target_count: u8,
    target_index: u8,
    phase: u8,
    state: u8,
}

const EMPTY_ECU_ID: u8 = 0xff;
const ECU_ID_ENGINE: u8 = 0;
const ECU_ID_BRAKE: u8 = 1;
const ECU_ID_STEERING: u8 = 2;
const PHASE_AWAITING_STAGE: u8 = 0;
const PHASE_AWAITING_TRIAL_HEALTH: u8 = 1;
const PHASE_AWAITING_COMMIT: u8 = 2;
const STATE_IDLE: u8 = 0;
const STATE_UPDATING: u8 = 1;
const STATE_COMPLETED: u8 = 2;
const STATE_FAILED: u8 = 3;

fn ecu_id(ecu: Ecu) -> u8 {
    match ecu {
        Ecu::Engine => ECU_ID_ENGINE,
        Ecu::Brake => ECU_ID_BRAKE,
        Ecu::Steering => ECU_ID_STEERING,
    }
}

fn ecu_from_id(id: u8) -> Option<Ecu> {
    match id {
        ECU_ID_ENGINE => Some(Ecu::Engine),
        ECU_ID_BRAKE => Some(Ecu::Brake),
        ECU_ID_STEERING => Some(Ecu::Steering),
        _ => None,
    }
}

impl CampaignManager {
    pub const fn new() -> Self {
        Self {
            targets: [EMPTY_ECU_ID; MAX_CAMPAIGN_ECUS],
            target_count: 0,
            target_index: 0,
            phase: PHASE_AWAITING_STAGE,
            state: STATE_IDLE,
        }
    }

    pub fn state(&self) -> CampaignState {
        match self.state {
            STATE_UPDATING => self
                .target_at(self.target_index)
                .map_or(CampaignState::Idle, |ecu| CampaignState::Updating { ecu }),
            STATE_COMPLETED => CampaignState::Completed,
            STATE_FAILED => self
                .target_at(self.target_index)
                .map_or(CampaignState::Idle, |ecu| CampaignState::Failed { ecu }),
            _ => CampaignState::Idle,
        }
    }

    /// Serialize campaign state to a stable, versioned record for an NvM adapter.
    ///
    /// The trailing CRC detects accidental corruption; it is not an authentication mechanism.
    pub fn encode_checkpoint(
        &self,
        output: &mut [u8; CAMPAIGN_CHECKPOINT_SIZE],
    ) -> Result<(), CampaignCheckpointError> {
        if !self.is_valid_checkpoint_state() {
            return Err(CampaignCheckpointError::InvalidState);
        }
        output.fill(0);
        output[CHECKPOINT_MAGIC_OFFSET..CHECKPOINT_VERSION_OFFSET]
            .copy_from_slice(&CHECKPOINT_MAGIC);
        output[CHECKPOINT_VERSION_OFFSET] = CHECKPOINT_VERSION;
        output[CHECKPOINT_TARGETS_OFFSET..CHECKPOINT_TARGET_COUNT_OFFSET]
            .copy_from_slice(&self.targets);
        output[CHECKPOINT_TARGET_COUNT_OFFSET] = self.target_count;
        output[CHECKPOINT_TARGET_INDEX_OFFSET] = self.target_index;
        output[CHECKPOINT_PHASE_OFFSET] = self.phase;
        output[CHECKPOINT_STATE_OFFSET] = self.state;
        let checksum = checkpoint_crc32(&output[..CHECKPOINT_CRC_OFFSET]);
        output[CHECKPOINT_CRC_OFFSET..].copy_from_slice(&checksum.to_le_bytes());
        Ok(())
    }

    /// Restore a campaign manager from a validated NvM record.
    pub fn from_checkpoint(checkpoint: &[u8]) -> Result<Self, CampaignCheckpointError> {
        if checkpoint.len() != CAMPAIGN_CHECKPOINT_SIZE {
            return Err(CampaignCheckpointError::InvalidLength);
        }
        if checkpoint[CHECKPOINT_MAGIC_OFFSET..CHECKPOINT_VERSION_OFFSET] != CHECKPOINT_MAGIC {
            return Err(CampaignCheckpointError::InvalidMagic);
        }
        if checkpoint[CHECKPOINT_VERSION_OFFSET] != CHECKPOINT_VERSION {
            return Err(CampaignCheckpointError::UnsupportedVersion);
        }
        let expected_crc = u32::from_le_bytes(
            checkpoint[CHECKPOINT_CRC_OFFSET..]
                .try_into()
                .map_err(|_| CampaignCheckpointError::InvalidLength)?,
        );
        if checkpoint_crc32(&checkpoint[..CHECKPOINT_CRC_OFFSET]) != expected_crc {
            return Err(CampaignCheckpointError::ChecksumMismatch);
        }

        let manager = Self {
            targets: checkpoint[CHECKPOINT_TARGETS_OFFSET..CHECKPOINT_TARGET_COUNT_OFFSET]
                .try_into()
                .map_err(|_| CampaignCheckpointError::InvalidLength)?,
            target_count: checkpoint[CHECKPOINT_TARGET_COUNT_OFFSET],
            target_index: checkpoint[CHECKPOINT_TARGET_INDEX_OFFSET],
            phase: checkpoint[CHECKPOINT_PHASE_OFFSET],
            state: checkpoint[CHECKPOINT_STATE_OFFSET],
        };
        if !manager.is_valid_checkpoint_state() {
            return Err(CampaignCheckpointError::InvalidState);
        }
        Ok(manager)
    }

    fn is_valid_checkpoint_state(&self) -> bool {
        let target_count = self.target_count as usize;
        if target_count > MAX_CAMPAIGN_ECUS || self.phase().is_none() {
            return false;
        }
        for index in 0..target_count {
            let Some(target) = self.target_at(index as u8) else {
                return false;
            };
            if (0..index).any(|previous| self.target_at(previous as u8) == Some(target)) {
                return false;
            }
        }
        if self.targets[target_count..]
            .iter()
            .any(|target| *target != EMPTY_ECU_ID)
        {
            return false;
        }

        match self.state {
            STATE_IDLE => {
                target_count == 0 && self.target_index == 0 && self.phase == PHASE_AWAITING_STAGE
            }
            STATE_UPDATING | STATE_FAILED => {
                target_count > 0 && (self.target_index as usize) < target_count
            }
            STATE_COMPLETED => target_count > 0 && self.target_index as usize + 1 == target_count,
            _ => false,
        }
    }

    fn target_at(&self, index: u8) -> Option<Ecu> {
        if index >= self.target_count || index as usize >= MAX_CAMPAIGN_ECUS {
            return None;
        }
        ecu_from_id(self.targets[index as usize])
    }

    fn phase(&self) -> Option<Phase> {
        match self.phase {
            PHASE_AWAITING_STAGE => Some(Phase::AwaitingStage),
            PHASE_AWAITING_TRIAL_HEALTH => Some(Phase::AwaitingTrialHealth),
            PHASE_AWAITING_COMMIT => Some(Phase::AwaitingCommit),
            _ => None,
        }
    }

    /// Recheck vehicle preconditions from the VCU's periodic task and abort an active target as
    /// soon as the vehicle moves, Safety leaves Normal, or another update becomes active.
    pub fn recheck(&mut self, conditions: VehicleConditions) -> Option<CampaignCommand> {
        let CampaignState::Updating { ecu } = self.state() else {
            return None;
        };
        if conditions.permits_update() {
            return None;
        }
        self.state = STATE_FAILED;
        Some(CampaignCommand::Abort(ecu))
    }

    /// Begin only when the vehicle is stopped, safety is normal, and no other update is running.
    pub fn start(
        &mut self,
        plan: CampaignPlan,
        conditions: VehicleConditions,
    ) -> Result<CampaignCommand, CampaignError> {
        if matches!(self.state(), CampaignState::Updating { .. }) {
            return Err(CampaignError::CampaignInProgress);
        }
        check_conditions(conditions)?;
        let first = plan.target(0).ok_or(CampaignError::EmptyPlan)?;
        self.targets = [EMPTY_ECU_ID; MAX_CAMPAIGN_ECUS];
        for index in 0..plan.len {
            if let Some(target) = plan.target(index) {
                self.targets[index as usize] = ecu_id(target);
            }
        }
        self.target_count = plan.len;
        self.target_index = 0;
        self.phase = PHASE_AWAITING_STAGE;
        self.state = STATE_UPDATING;
        Ok(CampaignCommand::Request(first))
    }

    /// Advance the campaign from a status reported by its currently selected ECU.
    pub fn report(
        &mut self,
        ecu: Ecu,
        status: EcuUpdateStatus,
        conditions: VehicleConditions,
    ) -> Result<Option<CampaignCommand>, CampaignError> {
        let CampaignState::Updating { ecu: expected } = self.state() else {
            return Err(CampaignError::NoCampaign);
        };
        if ecu != expected {
            return Err(CampaignError::UnexpectedEcu);
        }
        if let Some(command) = self.recheck(conditions) {
            return Ok(Some(command));
        }

        match (self.phase(), status) {
            (Some(Phase::AwaitingStage), EcuUpdateStatus::Staged) => {
                self.phase = PHASE_AWAITING_TRIAL_HEALTH;
                Ok(Some(CampaignCommand::Activate(ecu)))
            }
            (Some(Phase::AwaitingTrialHealth), EcuUpdateStatus::Staged) => {
                Ok(Some(CampaignCommand::Activate(ecu)))
            }
            (Some(Phase::AwaitingTrialHealth), EcuUpdateStatus::TrialHealthy) => {
                self.phase = PHASE_AWAITING_COMMIT;
                Ok(Some(CampaignCommand::Commit(ecu)))
            }
            (Some(Phase::AwaitingCommit), EcuUpdateStatus::TrialHealthy) => {
                Ok(Some(CampaignCommand::Commit(ecu)))
            }
            (Some(Phase::AwaitingCommit), EcuUpdateStatus::Committed) => {
                let next_index = self.target_index + 1;
                if let Some(next) = self.target_at(next_index) {
                    self.target_index = next_index;
                    self.phase = PHASE_AWAITING_STAGE;
                    self.state = STATE_UPDATING;
                    Ok(Some(CampaignCommand::Request(next)))
                } else {
                    self.state = STATE_COMPLETED;
                    Ok(None)
                }
            }
            (
                _,
                EcuUpdateStatus::Rejected | EcuUpdateStatus::RolledBack | EcuUpdateStatus::Failed,
            ) => {
                self.state = STATE_FAILED;
                Ok(Some(CampaignCommand::Abort(ecu)))
            }
            _ => Err(CampaignError::UnexpectedStatus),
        }
    }

    /// Cancel the active ECU update and stop the campaign.
    pub fn cancel(&mut self) -> Result<CampaignCommand, CampaignError> {
        let CampaignState::Updating { ecu } = self.state() else {
            return Err(CampaignError::NoCampaign);
        };
        self.state = STATE_FAILED;
        Ok(CampaignCommand::Abort(ecu))
    }
}

fn checkpoint_crc32(bytes: &[u8]) -> u32 {
    const CRC32_REFLECTED_POLYNOMIAL: u32 = 0xedb8_8320;

    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..u8::BITS {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ CRC32_REFLECTED_POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

impl Default for CampaignManager {
    fn default() -> Self {
        Self::new()
    }
}

fn check_conditions(conditions: VehicleConditions) -> Result<(), CampaignError> {
    if !conditions.stopped {
        return Err(CampaignError::VehicleMoving);
    }
    if conditions.safety != VehicleSafety::Normal {
        return Err(CampaignError::SafetyNotNormal);
    }
    if conditions.other_update_active {
        return Err(CampaignError::AnotherUpdateActive);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE: VehicleConditions = VehicleConditions {
        stopped: true,
        safety: VehicleSafety::Normal,
        other_update_active: false,
    };

    #[test]
    fn updates_targets_one_at_a_time_and_finishes_in_plan_order() {
        let plan = CampaignPlan::new(&[Ecu::Engine, Ecu::Brake, Ecu::Steering]).unwrap();
        let mut manager = CampaignManager::new();

        assert_eq!(
            manager.start(plan, SAFE),
            Ok(CampaignCommand::Request(Ecu::Engine))
        );
        assert_eq!(
            manager.report(Ecu::Engine, EcuUpdateStatus::Staged, SAFE),
            Ok(Some(CampaignCommand::Activate(Ecu::Engine)))
        );
        assert_eq!(
            manager.report(Ecu::Engine, EcuUpdateStatus::TrialHealthy, SAFE),
            Ok(Some(CampaignCommand::Commit(Ecu::Engine)))
        );
        assert_eq!(
            manager.report(Ecu::Engine, EcuUpdateStatus::Committed, SAFE),
            Ok(Some(CampaignCommand::Request(Ecu::Brake)))
        );
        assert_eq!(
            manager.report(Ecu::Brake, EcuUpdateStatus::Staged, SAFE),
            Ok(Some(CampaignCommand::Activate(Ecu::Brake)))
        );
        assert_eq!(
            manager.report(Ecu::Brake, EcuUpdateStatus::TrialHealthy, SAFE),
            Ok(Some(CampaignCommand::Commit(Ecu::Brake)))
        );
        assert_eq!(
            manager.report(Ecu::Brake, EcuUpdateStatus::Committed, SAFE),
            Ok(Some(CampaignCommand::Request(Ecu::Steering)))
        );
        assert_eq!(
            manager.report(Ecu::Steering, EcuUpdateStatus::Staged, SAFE),
            Ok(Some(CampaignCommand::Activate(Ecu::Steering)))
        );
        assert_eq!(
            manager.report(Ecu::Steering, EcuUpdateStatus::TrialHealthy, SAFE),
            Ok(Some(CampaignCommand::Commit(Ecu::Steering)))
        );
        assert_eq!(
            manager.report(Ecu::Steering, EcuUpdateStatus::Committed, SAFE),
            Ok(None)
        );
        assert_eq!(manager.state(), CampaignState::Completed);
    }

    #[test]
    fn refuses_campaigns_when_vehicle_or_safety_preconditions_fail() {
        let plan = CampaignPlan::new(&[Ecu::Engine]).unwrap();
        let mut manager = CampaignManager::new();
        assert_eq!(
            manager.start(
                plan,
                VehicleConditions {
                    stopped: false,
                    ..SAFE
                }
            ),
            Err(CampaignError::VehicleMoving)
        );
        assert_eq!(
            manager.start(
                plan,
                VehicleConditions {
                    safety: VehicleSafety::Degraded,
                    ..SAFE
                }
            ),
            Err(CampaignError::SafetyNotNormal)
        );
        assert_eq!(
            manager.start(
                plan,
                VehicleConditions {
                    other_update_active: true,
                    ..SAFE
                }
            ),
            Err(CampaignError::AnotherUpdateActive)
        );
    }

    #[test]
    fn failure_or_lost_preconditions_abort_without_starting_next_ecu() {
        let plan = CampaignPlan::new(&[Ecu::Engine, Ecu::Brake]).unwrap();
        let mut manager = CampaignManager::new();
        manager.start(plan, SAFE).unwrap();
        assert_eq!(
            manager.report(
                Ecu::Engine,
                EcuUpdateStatus::Staged,
                VehicleConditions {
                    safety: VehicleSafety::EStop,
                    ..SAFE
                }
            ),
            Ok(Some(CampaignCommand::Abort(Ecu::Engine)))
        );
        assert_eq!(manager.state(), CampaignState::Failed { ecu: Ecu::Engine });
    }

    #[test]
    fn periodic_precondition_check_aborts_without_waiting_for_ecu_status() {
        let plan = CampaignPlan::new(&[Ecu::Engine, Ecu::Brake]).unwrap();
        let mut manager = CampaignManager::new();
        manager.start(plan, SAFE).unwrap();
        assert_eq!(
            manager.recheck(VehicleConditions {
                stopped: false,
                ..SAFE
            }),
            Some(CampaignCommand::Abort(Ecu::Engine))
        );
        assert_eq!(manager.state(), CampaignState::Failed { ecu: Ecu::Engine });
        assert_eq!(
            manager.recheck(SAFE),
            None,
            "the terminal failed campaign must not emit duplicate aborts"
        );
    }

    #[test]
    fn rejects_invalid_plans_and_out_of_order_status() {
        assert_eq!(CampaignPlan::new(&[]), Err(CampaignError::EmptyPlan));
        assert_eq!(
            CampaignPlan::new(&[Ecu::Engine, Ecu::Engine]),
            Err(CampaignError::DuplicateTarget)
        );
        assert_eq!(
            CampaignPlan::new(&[Ecu::Engine, Ecu::Brake, Ecu::Steering, Ecu::Engine]),
            Err(CampaignError::TooManyTargets)
        );
        let plan = CampaignPlan::new(&[Ecu::Engine]).unwrap();
        let mut manager = CampaignManager::new();
        manager.start(plan, SAFE).unwrap();
        assert_eq!(
            manager.start(plan, SAFE),
            Err(CampaignError::CampaignInProgress)
        );
        assert_eq!(
            manager.report(Ecu::Brake, EcuUpdateStatus::Staged, SAFE),
            Err(CampaignError::UnexpectedEcu)
        );
        assert_eq!(
            manager.report(Ecu::Engine, EcuUpdateStatus::TrialHealthy, SAFE),
            Err(CampaignError::UnexpectedStatus)
        );
        assert_eq!(manager.cancel(), Ok(CampaignCommand::Abort(Ecu::Engine)));
    }

    #[test]
    fn checkpoint_round_trip_preserves_active_plan_and_phase() {
        let plan = CampaignPlan::new(&[Ecu::Engine, Ecu::Brake]).unwrap();
        let mut manager = CampaignManager::new();
        manager.start(plan, SAFE).unwrap();
        manager
            .report(Ecu::Engine, EcuUpdateStatus::Staged, SAFE)
            .unwrap();

        let mut checkpoint = [0; CAMPAIGN_CHECKPOINT_SIZE];
        manager.encode_checkpoint(&mut checkpoint).unwrap();
        let mut restored = CampaignManager::from_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            restored.state(),
            CampaignState::Updating { ecu: Ecu::Engine }
        );
        assert_eq!(
            restored.report(Ecu::Engine, EcuUpdateStatus::Staged, SAFE),
            Ok(Some(CampaignCommand::Activate(Ecu::Engine))),
            "if reset happened before Activate reached the ECU, reconciliation reissues it"
        );
        restored.encode_checkpoint(&mut checkpoint).unwrap();
        let mut restored = CampaignManager::from_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            restored.report(Ecu::Engine, EcuUpdateStatus::TrialHealthy, SAFE),
            Ok(Some(CampaignCommand::Commit(Ecu::Engine)))
        );
        assert_eq!(
            restored.report(Ecu::Engine, EcuUpdateStatus::TrialHealthy, SAFE),
            Ok(Some(CampaignCommand::Commit(Ecu::Engine))),
            "if reset happened before Commit reached the ECU, reconciliation reissues it"
        );
        assert_eq!(
            restored.report(Ecu::Engine, EcuUpdateStatus::Committed, SAFE),
            Ok(Some(CampaignCommand::Request(Ecu::Brake)))
        );
    }

    #[test]
    fn checkpoint_rejects_wrong_length_version_corruption_and_invalid_state() {
        let mut manager = CampaignManager::new();
        let mut checkpoint = [0; CAMPAIGN_CHECKPOINT_SIZE];
        manager.encode_checkpoint(&mut checkpoint).unwrap();

        assert_eq!(
            CampaignManager::from_checkpoint(&checkpoint[..checkpoint.len() - 1]),
            Err(CampaignCheckpointError::InvalidLength)
        );
        let mut unsupported_version = checkpoint;
        unsupported_version[CHECKPOINT_VERSION_OFFSET] = CHECKPOINT_VERSION + 1;
        assert_eq!(
            CampaignManager::from_checkpoint(&unsupported_version),
            Err(CampaignCheckpointError::UnsupportedVersion)
        );
        let mut corrupted = checkpoint;
        corrupted[CHECKPOINT_STATE_OFFSET] = STATE_COMPLETED;
        assert_eq!(
            CampaignManager::from_checkpoint(&corrupted),
            Err(CampaignCheckpointError::ChecksumMismatch)
        );

        let plan = CampaignPlan::new(&[Ecu::Engine]).unwrap();
        manager.start(plan, SAFE).unwrap();
        manager.encode_checkpoint(&mut checkpoint).unwrap();
        checkpoint[CHECKPOINT_TARGET_INDEX_OFFSET] = 1;
        let checksum = checkpoint_crc32(&checkpoint[..CHECKPOINT_CRC_OFFSET]);
        checkpoint[CHECKPOINT_CRC_OFFSET..].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(
            CampaignManager::from_checkpoint(&checkpoint),
            Err(CampaignCheckpointError::InvalidState)
        );
    }
}
