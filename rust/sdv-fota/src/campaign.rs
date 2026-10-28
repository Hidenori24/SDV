//! VCU-side policy for running a multi-ECU update campaign sequentially.
//!
//! This allocator-free state machine emits commands for a diagnostic or Classic BSW adapter to
//! transport. It does not implement a vehicle communication protocol.

use crate::Ecu;

pub const MAX_CAMPAIGN_ECUS: usize = 3;

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

/// Serial campaign controller. It never has more than one ECU request outstanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CampaignManager {
    plan: Option<CampaignPlan>,
    target_index: u8,
    phase: Phase,
    state: CampaignState,
}

impl CampaignManager {
    pub const fn new() -> Self {
        Self {
            plan: None,
            target_index: 0,
            phase: Phase::AwaitingStage,
            state: CampaignState::Idle,
        }
    }

    pub fn state(&self) -> CampaignState {
        self.state
    }

    /// Recheck vehicle preconditions from the VCU's periodic task and abort an active target as
    /// soon as the vehicle moves, Safety leaves Normal, or another update becomes active.
    pub fn recheck(&mut self, conditions: VehicleConditions) -> Option<CampaignCommand> {
        let CampaignState::Updating { ecu } = self.state else {
            return None;
        };
        if conditions.permits_update() {
            return None;
        }
        self.state = CampaignState::Failed { ecu };
        Some(CampaignCommand::Abort(ecu))
    }

    /// Begin only when the vehicle is stopped, safety is normal, and no other update is running.
    pub fn start(
        &mut self,
        plan: CampaignPlan,
        conditions: VehicleConditions,
    ) -> Result<CampaignCommand, CampaignError> {
        if matches!(self.state, CampaignState::Updating { .. }) {
            return Err(CampaignError::CampaignInProgress);
        }
        check_conditions(conditions)?;
        let first = plan.target(0).ok_or(CampaignError::EmptyPlan)?;
        self.plan = Some(plan);
        self.target_index = 0;
        self.phase = Phase::AwaitingStage;
        self.state = CampaignState::Updating { ecu: first };
        Ok(CampaignCommand::Request(first))
    }

    /// Advance the campaign from a status reported by its currently selected ECU.
    pub fn report(
        &mut self,
        ecu: Ecu,
        status: EcuUpdateStatus,
        conditions: VehicleConditions,
    ) -> Result<Option<CampaignCommand>, CampaignError> {
        let CampaignState::Updating { ecu: expected } = self.state else {
            return Err(CampaignError::NoCampaign);
        };
        if ecu != expected {
            return Err(CampaignError::UnexpectedEcu);
        }
        if let Some(command) = self.recheck(conditions) {
            return Ok(Some(command));
        }

        match (self.phase, status) {
            (Phase::AwaitingStage, EcuUpdateStatus::Staged) => {
                self.phase = Phase::AwaitingTrialHealth;
                Ok(Some(CampaignCommand::Activate(ecu)))
            }
            (Phase::AwaitingTrialHealth, EcuUpdateStatus::TrialHealthy) => {
                self.phase = Phase::AwaitingCommit;
                Ok(Some(CampaignCommand::Commit(ecu)))
            }
            (Phase::AwaitingCommit, EcuUpdateStatus::Committed) => {
                let next_index = self.target_index + 1;
                let Some(plan) = self.plan else {
                    return Err(CampaignError::NoCampaign);
                };
                if let Some(next) = plan.target(next_index) {
                    self.target_index = next_index;
                    self.phase = Phase::AwaitingStage;
                    self.state = CampaignState::Updating { ecu: next };
                    Ok(Some(CampaignCommand::Request(next)))
                } else {
                    self.state = CampaignState::Completed;
                    Ok(None)
                }
            }
            (
                _,
                EcuUpdateStatus::Rejected | EcuUpdateStatus::RolledBack | EcuUpdateStatus::Failed,
            ) => {
                self.state = CampaignState::Failed { ecu };
                Ok(Some(CampaignCommand::Abort(ecu)))
            }
            _ => Err(CampaignError::UnexpectedStatus),
        }
    }

    /// Cancel the active ECU update and stop the campaign.
    pub fn cancel(&mut self) -> Result<CampaignCommand, CampaignError> {
        let CampaignState::Updating { ecu } = self.state else {
            return Err(CampaignError::NoCampaign);
        };
        self.state = CampaignState::Failed { ecu };
        Ok(CampaignCommand::Abort(ecu))
    }
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
        let plan = CampaignPlan::new(&[Ecu::Engine]).unwrap();
        let mut manager = CampaignManager::new();
        manager.start(plan, SAFE).unwrap();
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
}
