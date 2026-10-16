//! Fixed-step scheduler + minimal RTE, mirroring the C++ `main.cpp` task order:
//! 10 ms: Engine -> Brake -> Steering -> VehicleDynamics -> log,
//! then 20 ms: DriverInput, 100 ms: Safety.

use crate::model::*;

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

pub const DT_10MS: f32 = 0.010;

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
        }
    }

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

        // Engine, Brake
        self.cmd.drive_accel_cmd = compute_drive_accel(self.input.throttle, estop, &self.engine);
        self.cmd.brake_decel_cmd = compute_brake_decel(self.input.brake, estop, &self.brake);

        // Steering
        let target = compute_steer_target(self.input.steer, estop, &self.steering);
        self.steer_angle = step_steering_dynamics(self.steer_angle, target, DT_10MS, &self.steering);
        self.cmd.steer_angle_cmd = self.steer_angle;

        // VehicleDynamics
        self.state = step_vehicle(
            &self.state, DT_10MS,
            self.cmd.drive_accel_cmd, self.cmd.brake_decel_cmd, self.cmd.steer_angle_cmd,
            estop, &self.vehicle,
        );

        let row = Row { input: self.input, cmd: self.cmd, state: self.state, safety: self.safety };

        if i % 2 == 0 && self.use_scenario {
            self.input = Self::scenario_input(self.state.t);
        }
        if i % 10 == 0 {
            self.safety.system_state =
                if self.safety.estop { SystemState::EStop } else { SystemState::Normal };
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
