//! Fixed-step scheduler + minimal RTE, mirroring the C++ `main.cpp` task order:
//! 10 ms: Engine -> Brake -> Steering -> VehicleDynamics -> log,
//! then 20 ms: DriverInput, 100 ms: Safety.

use crate::model::*;
use crate::safety::{SafetySupervisor, SwcStatus};

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

/// One row of the simulation log (same columns as the C++ CSV).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Row {
    pub input: DriverInput,
    pub cmd: ActuatorCmd,
    pub state: VehicleState,
    pub safety: Safety,
}

impl Row {
    pub const CSV_HEADER: &'static str = "t,throttle,brake,steer,drive_accel_cmd,brake_decel_cmd,steer_angle_cmd,x,y,yaw,v,yaw_rate,wheel_omega,estop,system_state";

    pub fn to_csv(&self) -> String {
        format!(
            "{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.6},{:.3},{:.3},{:.6},{:.3},{:.6},{:.3},{},{}",
            self.state.t, self.input.throttle, self.input.brake, self.input.steer,
            self.cmd.drive_accel_cmd, self.cmd.brake_decel_cmd, self.cmd.steer_angle_cmd,
            self.state.x, self.state.y, self.state.yaw, self.state.v,
            self.state.yaw_rate, self.state.wheel_omega,
            self.safety.estop as u8, self.safety.system_state as u8,
        )
    }
}

/// SWCs supervised by the watchdog (index = position in the alive-counter array).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Component {
    Engine = 0,
    Brake = 1,
    Steering = 2,
    VehicleDynamics = 3,
}

pub const DT_10MS: f32 = 0.010;
pub const DT_100MS_MS: u32 = 100;

pub struct Sim {
    pub input: DriverInput,
    pub cmd: ActuatorCmd,
    pub state: VehicleState,
    pub safety: Safety,
    /// true: DriverInput follows the built-in demo scenario (like the C++ app);
    /// false: `input` is controlled from outside (GUI).
    pub use_scenario: bool,
    engine: EngineParams,
    brake: BrakeParams,
    steering: SteeringParams,
    vehicle: VehicleParams,
    steer_angle: f32,
    tick: u64,
    supervisor: SafetySupervisor,
    /// Alive counters (bumped by each SWC when it runs), like `Bsw::Diag::ReportAlive`.
    alive: [u64; 4],
    /// Fault injection: a stalled SWC is not executed (and therefore stops reporting alive).
    stalled: [bool; 4],
}

impl Sim {
    pub fn new() -> Self {
        Self {
            input: DriverInput::default(),
            cmd: ActuatorCmd::default(),
            state: VehicleState::default(),
            safety: Safety::default(),
            use_scenario: false,
            engine: EngineParams::default(),
            brake: BrakeParams::default(),
            steering: SteeringParams::default(),
            vehicle: VehicleParams::default(),
            steer_angle: 0.0,
            tick: 0,
            supervisor: SafetySupervisor::new(),
            alive: [0; 4],
            stalled: [false; 4],
        }
    }

    /// Fault injection (like `sdv_sim --stall`): stop (`true`) or resume (`false`) executing a SWC.
    pub fn set_stall(&mut self, c: Component, stalled: bool) {
        self.stalled[c as usize] = stalled;
    }

    /// External E-Stop request (UI / hardware); read directly by the SWCs like the RTE `estop` flag.
    pub fn set_estop_request(&mut self, on: bool) {
        self.safety.estop = on;
    }

    /// Watchdog view of each SWC: [engine, brake, steering, vehicledynamics].
    pub fn swc_status(&self) -> [SwcStatus; 4] {
        self.supervisor.status()
    }

    /// Reset everything (this releases a latched watchdog EStop and clears injected stalls).
    pub fn reset(&mut self) {
        let use_scenario = self.use_scenario;
        *self = Self::new();
        self.use_scenario = use_scenario;
    }

    fn estop(&self) -> bool {
        self.safety.estop || self.safety.system_state == SystemState::EStop
    }

    /// Built-in demo scenario (same as C++ DriverInputSWC).
    fn scenario_input(t: f32) -> DriverInput {
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

    /// Advance one 10 ms tick and return the log row.
    pub fn step_10ms(&mut self) -> Row {
        let i = self.tick;
        let estop = self.estop();

        // A stalled SWC neither updates its outputs nor reports alive.
        // Engine
        if !self.stalled[Component::Engine as usize] {
            self.cmd.drive_accel_cmd = compute_drive_accel(self.input.throttle, estop, &self.engine);
            self.alive[Component::Engine as usize] += 1;
        }
        // Brake
        if !self.stalled[Component::Brake as usize] {
            self.cmd.brake_decel_cmd = compute_brake_decel(self.input.brake, estop, &self.brake);
            self.alive[Component::Brake as usize] += 1;
        }
        // Steering
        if !self.stalled[Component::Steering as usize] {
            let target = compute_steer_target(self.input.steer, estop, &self.steering);
            self.steer_angle = step_steering_dynamics(self.steer_angle, target, DT_10MS, &self.steering);
            self.cmd.steer_angle_cmd = self.steer_angle;
            self.alive[Component::Steering as usize] += 1;
        }
        // VehicleDynamics (a stalled plant also freezes simulated time, as in C++)
        if !self.stalled[Component::VehicleDynamics as usize] {
            self.state = step_vehicle(
                &self.state, DT_10MS,
                self.cmd.drive_accel_cmd, self.cmd.brake_decel_cmd, self.cmd.steer_angle_cmd,
                estop, &self.vehicle,
            );
            self.alive[Component::VehicleDynamics as usize] += 1;
        }

        let row = Row { input: self.input, cmd: self.cmd, state: self.state, safety: self.safety };

        if i % 2 == 0 && self.use_scenario {
            self.input = Self::scenario_input(self.state.t);
        }
        if i % 10 == 0 {
            // Safety SWC: `safety.estop` holds the external request; the watchdog latch is OR-ed in.
            let (estop, state) = self.supervisor.main_100ms(self.alive, DT_100MS_MS, self.safety.estop);
            self.safety.estop = estop;
            self.safety.system_state = state;
        }
        self.tick += 1;
        row
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}
