//! Simulator adapter for the platform-independent Classic FOTA campaign policy.
//!
//! This binds campaign start/recheck conditions to simulated vehicle speed and VCU Safety state.
//! ECU flashing still requires a target diagnostic/communication adapter.

use sdv_fota::{
    CampaignCommand, CampaignError, CampaignManager, CampaignPlan, CampaignState, Ecu,
    EcuUpdateStatus, VehicleConditions, VehicleSafety,
};

use crate::sim::{Sim, SystemState};

/// Tolerance for treating floating point vehicle speed as stopped.
pub const STOPPED_SPEED_EPSILON_MPS: f32 = 0.01;

/// VCU-side campaign policy bound to the live simulation state.
#[derive(Debug)]
pub struct SimFotaCampaign {
    manager: CampaignManager,
}

impl SimFotaCampaign {
    pub const fn new() -> Self {
        Self {
            manager: CampaignManager::new(),
        }
    }

    pub fn state(&self) -> CampaignState {
        self.manager.state()
    }

    /// Start an ECU campaign using the current simulated vehicle and Safety conditions.
    pub fn start(
        &mut self,
        sim: &Sim,
        targets: &[Ecu],
        other_update_active: bool,
    ) -> Result<CampaignCommand, CampaignError> {
        let plan = CampaignPlan::new(targets)?;
        self.manager
            .start(plan, vehicle_conditions(sim, other_update_active))
    }

    /// Feed an authoritative ECU status into the campaign policy using current vehicle state.
    pub fn report(
        &mut self,
        sim: &Sim,
        ecu: Ecu,
        status: EcuUpdateStatus,
        other_update_active: bool,
    ) -> Result<Option<CampaignCommand>, CampaignError> {
        self.manager
            .report(ecu, status, vehicle_conditions(sim, other_update_active))
    }

    /// Recheck conditions from the simulator's VCU periodic task.
    pub fn recheck(&mut self, sim: &Sim, other_update_active: bool) -> Option<CampaignCommand> {
        self.manager
            .recheck(vehicle_conditions(sim, other_update_active))
    }

    pub fn cancel(&mut self) -> Result<CampaignCommand, CampaignError> {
        self.manager.cancel()
    }
}

impl Default for SimFotaCampaign {
    fn default() -> Self {
        Self::new()
    }
}

fn vehicle_conditions(sim: &Sim, other_update_active: bool) -> VehicleConditions {
    let safety = sim.safety();
    let safety_state = if safety.estop {
        VehicleSafety::EStop
    } else {
        match safety.system_state {
            SystemState::Normal => VehicleSafety::Normal,
            SystemState::Degraded => VehicleSafety::Degraded,
            SystemState::EStop => VehicleSafety::EStop,
        }
    };

    VehicleConditions {
        stopped: sim.state().v.abs() <= STOPPED_SPEED_EPSILON_MPS,
        safety: safety_state,
        other_update_active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_start_uses_vehicle_speed_and_safety_state() {
        let mut sim = Sim::new();
        let mut campaign = SimFotaCampaign::new();

        assert_eq!(
            campaign.start(&sim, &[Ecu::Engine], false),
            Ok(CampaignCommand::Request(Ecu::Engine))
        );
        campaign.cancel().unwrap();

        sim.input_mut().throttle = 1.0;
        for _ in 0..100 {
            sim.step_10ms();
        }
        assert!(sim.state().v > STOPPED_SPEED_EPSILON_MPS);
        assert_eq!(
            campaign.start(&sim, &[Ecu::Engine], false),
            Err(CampaignError::VehicleMoving)
        );

        let mut stopped_sim = Sim::new();
        stopped_sim.set_estop_request(true);
        stopped_sim.step_10ms();
        assert_eq!(
            campaign.start(&stopped_sim, &[Ecu::Engine], false),
            Err(CampaignError::SafetyNotNormal)
        );
        assert_eq!(
            campaign.start(&Sim::new(), &[Ecu::Engine], true),
            Err(CampaignError::AnotherUpdateActive)
        );
    }

    #[test]
    fn periodic_recheck_aborts_when_simulated_vehicle_starts_moving() {
        let mut sim = Sim::new();
        let mut campaign = SimFotaCampaign::new();
        campaign
            .start(&sim, &[Ecu::Engine, Ecu::Brake], false)
            .unwrap();

        sim.input_mut().throttle = 1.0;
        for _ in 0..100 {
            sim.step_10ms();
        }
        assert_eq!(
            campaign.recheck(&sim, false),
            Some(CampaignCommand::Abort(Ecu::Engine))
        );
        assert_eq!(campaign.state(), CampaignState::Failed { ecu: Ecu::Engine });
    }

    #[test]
    fn periodic_recheck_aborts_when_simulated_safety_enters_estop() {
        let mut sim = Sim::new();
        let mut campaign = SimFotaCampaign::new();
        campaign.start(&sim, &[Ecu::Brake], false).unwrap();

        sim.set_estop_request(true);
        sim.step_10ms();
        assert_eq!(sim.safety().system_state, SystemState::EStop);
        assert_eq!(
            campaign.recheck(&sim, false),
            Some(CampaignCommand::Abort(Ecu::Brake))
        );
    }
}
