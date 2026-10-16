//! Software components. Each SWC reads and writes only the `Rte` it is given
//! (mirrors `src/swc/*.cpp`).

use crate::model::*;
use crate::rte::{DriverInput, Rte};
use crate::safety::SafetySupervisor;

#[derive(Default)]
pub struct EngineSwc {
    pub params: EngineParams,
}
impl EngineSwc {
    pub fn main_10ms(&self, rte: &mut Rte) {
        let estop = rte.estop_active();
        rte.cmd.drive_accel_cmd = compute_drive_accel(rte.input.throttle, estop, &self.params);
    }
}

#[derive(Default)]
pub struct BrakeSwc {
    pub params: BrakeParams,
}
impl BrakeSwc {
    pub fn main_10ms(&self, rte: &mut Rte) {
        let estop = rte.estop_active();
        rte.cmd.brake_decel_cmd = compute_brake_decel(rte.input.brake, estop, &self.params);
    }
}

#[derive(Default)]
pub struct SteeringSwc {
    pub params: SteeringParams,
    angle: f32,
}
impl SteeringSwc {
    pub fn main_10ms(&mut self, rte: &mut Rte, dt_s: f32) {
        let target = compute_steer_target(rte.input.steer, rte.estop_active(), &self.params);
        self.angle = step_steering_dynamics(self.angle, target, dt_s, &self.params);
        rte.cmd.steer_angle_cmd = self.angle;
    }
}

/// The plant (physical world). Not an ECU.
#[derive(Default)]
pub struct VehicleDynamicsSwc {
    pub params: VehicleParams,
}
impl VehicleDynamicsSwc {
    pub fn step_10ms(&self, rte: &mut Rte, dt_s: f32) {
        rte.state = step_vehicle(
            &rte.state, dt_s,
            rte.cmd.drive_accel_cmd, rte.cmd.brake_decel_cmd, rte.cmd.steer_angle_cmd,
            rte.estop_active(), &self.params,
        );
    }
}

/// Built-in demo scenario (same as C++ DriverInputSWC).
pub struct DriverInputSwc;
impl DriverInputSwc {
    pub fn scenario_input(t: f32) -> DriverInput {
        let (throttle, brake, steer) = if t < 2.0 {
            (0.6, 0.0, 0.0)
        } else if t < 5.0 {
            (0.2, 0.0, 0.2)
        } else if t < 7.0 {
            (0.0, 0.6, 0.0)
        } else {
            (0.0, 0.0, 0.0)
        };
        DriverInput {
            throttle: f32::clamp(throttle, 0.0, 1.0),
            brake: f32::clamp(brake, 0.0, 1.0),
            steer: f32::clamp(steer, -1.0, 1.0),
        }
    }

    /// 20 ms task, scenario mode: drive the inputs from the simulated time.
    pub fn main_20ms_scenario(rte: &mut Rte) {
        rte.input = Self::scenario_input(rte.state.t);
    }
}

#[derive(Default)]
pub struct SafetySwc {
    pub supervisor: SafetySupervisor,
}
impl SafetySwc {
    /// 100 ms task. `rte.safety.estop` holds the external request; the watchdog latch is OR-ed in.
    pub fn main_100ms(&mut self, rte: &mut Rte, alive: [u64; 4], dt_ms: u32) {
        let (estop, state) = self.supervisor.main_100ms(alive, dt_ms, rte.safety.estop);
        rte.safety.estop = estop;
        rte.safety.system_state = state;
    }
}
