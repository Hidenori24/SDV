//! Fixed-step scheduler + minimal RTE, mirroring the C++ `main.cpp` task order:
//! 10 ms: Engine -> Brake -> Steering -> VehicleDynamics -> log,
//! then 20 ms: DriverInput, 100 ms: Safety.

use crate::model::*;
use crate::rte::Rte;
use crate::safety::SwcStatus;
use crate::swc::*;

pub use crate::rte::{ActuatorCmd, DriverInput, Safety, SystemState};

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
    rte: Rte,
    /// true: DriverInput follows the built-in demo scenario (like the C++ app);
    /// false: the input is controlled from outside (GUI).
    use_scenario: bool,
    engine: EngineSwc,
    brake: BrakeSwc,
    steering: SteeringSwc,
    vehicle: VehicleDynamicsSwc,
    safety_swc: SafetySwc,
    tick: u64,
    /// Alive counters (bumped by each SWC when it runs), like `Bsw::Diag::ReportAlive`.
    alive: [u64; 4],
    /// Fault injection: a stalled SWC is not executed (and therefore stops reporting alive).
    stalled: [bool; 4],
}

impl Sim {
    pub fn new() -> Self {
        Self {
            rte: Rte::default(),
            use_scenario: false,
            engine: EngineSwc::default(),
            brake: BrakeSwc::default(),
            steering: SteeringSwc::default(),
            vehicle: VehicleDynamicsSwc::default(),
            safety_swc: SafetySwc::default(),
            tick: 0,
            alive: [0; 4],
            stalled: [false; 4],
        }
    }

    // ---- signal access (the GUI / tests see the system through these) ----
    pub fn input(&self) -> DriverInput { self.rte.input }
    pub fn input_mut(&mut self) -> &mut DriverInput { &mut self.rte.input }
    pub fn cmd(&self) -> ActuatorCmd { self.rte.cmd }
    pub fn state(&self) -> VehicleState { self.rte.state }
    pub fn safety(&self) -> Safety { self.rte.safety }
    pub fn set_use_scenario(&mut self, on: bool) { self.use_scenario = on; }

    /// Fault injection (like `sdv_sim --stall`): stop (`true`) or resume (`false`) executing a SWC.
    pub fn set_stall(&mut self, c: Component, stalled: bool) {
        self.stalled[c as usize] = stalled;
    }

    /// External E-Stop request (UI / hardware); read directly by the SWCs like the RTE `estop` flag.
    pub fn set_estop_request(&mut self, on: bool) {
        self.rte.safety.estop = on;
    }

    /// Watchdog view of each SWC: [engine, brake, steering, vehicledynamics].
    pub fn swc_status(&self) -> [SwcStatus; 4] {
        self.safety_swc.supervisor.status()
    }

    /// Reset everything (this releases a latched watchdog EStop and clears injected stalls).
    pub fn reset(&mut self) {
        let use_scenario = self.use_scenario;
        *self = Self::new();
        self.use_scenario = use_scenario;
    }

    /// Advance one 10 ms tick and return the log row.
    pub fn step_10ms(&mut self) -> Row {
        let i = self.tick;
        let dt = DT_10MS;

        // A stalled SWC neither updates its outputs nor reports alive.
        if !self.stalled[Component::Engine as usize] {
            self.engine.main_10ms(&mut self.rte);
            self.alive[Component::Engine as usize] += 1;
        }
        if !self.stalled[Component::Brake as usize] {
            self.brake.main_10ms(&mut self.rte);
            self.alive[Component::Brake as usize] += 1;
        }
        if !self.stalled[Component::Steering as usize] {
            self.steering.main_10ms(&mut self.rte, dt);
            self.alive[Component::Steering as usize] += 1;
        }
        // A stalled plant also freezes simulated time, as in C++.
        if !self.stalled[Component::VehicleDynamics as usize] {
            self.vehicle.step_10ms(&mut self.rte, dt);
            self.alive[Component::VehicleDynamics as usize] += 1;
        }

        let row = Row {
            input: self.rte.input,
            cmd: self.rte.cmd,
            state: self.rte.state,
            safety: self.rte.safety,
        };

        if i % 2 == 0 && self.use_scenario {
            DriverInputSwc::main_20ms_scenario(&mut self.rte);
        }
        if i % 10 == 0 {
            self.safety_swc.main_100ms(&mut self.rte, self.alive, DT_100MS_MS);
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
