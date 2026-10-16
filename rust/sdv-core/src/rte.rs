//! Signal store ("RTE") shared by the SWCs that run on the same ECU.
//!
//! One `Rte` instance per ECU; SWCs only talk to each other through it
//! (`Rte_Read_*` / `Rte_Write_*` in the C++ code base).

use crate::model::VehicleState;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriverInput {
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActuatorCmd {
    pub drive_accel_cmd: f32,
    pub brake_decel_cmd: f32,
    pub steer_angle_cmd: f32,
}

/// Same values as `model::SafetyState`.
pub use crate::model::SafetyState as SystemState;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Safety {
    pub estop: bool,
    pub system_state: SystemState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rte {
    pub input: DriverInput,
    pub cmd: ActuatorCmd,
    pub state: VehicleState,
    pub safety: Safety,
    /// Alive counters received over the bus: [engine, brake, steering, vehicledynamics].
    /// Only the node that supervises them (VCU) uses this.
    pub alive_rx: [u64; 4],
    /// Ticks (10 ms) since the last DriverInput / Safety frame was received (for the fail-safe).
    pub rx_age: RxAge,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxAge {
    pub driver: u32,
    pub safety: u32,
}

impl Rte {
    /// E-Stop is active when requested externally or when the system state says so.
    pub fn estop_active(&self) -> bool {
        self.safety.estop || self.safety.system_state == SystemState::EStop
    }
}
