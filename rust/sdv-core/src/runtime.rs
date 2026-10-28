//! Virtual ECU task runtime for a vehicle platform.
//!
//! The VCU, Engine, Brake, and Steering tasks keep independent RTE instances and exchange
//! signals through the same bus contract used by [`crate::sim::Sim`]. A host platform calls
//! [`VehicleRuntime::step_10ms`] from its 10 ms schedule, supplies input and sensor data, then
//! applies the returned actuator commands. The simulator's Plant is intentionally outside this
//! runtime; a real platform supplies its own I/O adapter.

use crate::bus::{
    Bus, Frame, Node, ALIVE_BRAKE_INDEX, ALIVE_ENGINE_INDEX, ALIVE_PLANT_INDEX,
    ALIVE_STEERING_INDEX,
};
use crate::config::Config;
use crate::failsafe::{self, FailsafeParams};
use crate::model::VehicleState;
use crate::rte::{ActuatorCmd, DriverInput, Rte, Safety};
use crate::swc::{BrakeSwc, EngineSwc, SafetySwc, SteeringSwc};

const ECU_STEP_S: f32 = 0.010;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualEcuId {
    Engine,
    Brake,
    Steering,
}

struct EcuTask<S> {
    rte: Rte,
    swc: S,
    alive: u64,
    enabled: bool,
}

impl<S> EcuTask<S> {
    fn new(swc: S) -> Self {
        Self {
            rte: Rte::default(),
            swc,
            alive: 0,
            enabled: true,
        }
    }
}

struct VcuTask {
    rte: Rte,
    swc: SafetySwc,
    enabled: bool,
}

/// Outputs produced by the virtual ECU tasks for a vehicle platform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleActuation {
    /// Command to apply to the physical actuators.
    pub command: ActuatorCmd,
    /// Safety state for the platform's independent E-stop path.
    pub safety: Safety,
}

/// Runs virtual ECU tasks in deterministic bus order, without simulating the physical plant.
///
/// This is one Linux process with separate ECU task state, not a claim that the ECUs have
/// independent CPU or memory protection. Call `step_10ms` at a stable 10 ms cadence. The platform
/// adapter supplies measured/estimated vehicle state and reports whether its I/O path is alive.
pub struct VehicleRuntime {
    vcu: VcuTask,
    engine: EcuTask<EngineSwc>,
    brake: EcuTask<BrakeSwc>,
    steering: EcuTask<SteeringSwc>,
    plant_rte: Rte,
    plant_alive: u64,
    bus: Bus,
    failsafe: FailsafeParams,
    tick: u64,
}

impl VehicleRuntime {
    pub fn new(config: Config) -> Self {
        Self {
            vcu: VcuTask {
                rte: Rte::default(),
                swc: SafetySwc::new(config.safety_params()),
                enabled: true,
            },
            engine: EcuTask::new(EngineSwc::new(config.engine_params())),
            brake: EcuTask::new(BrakeSwc::new(config.brake_params())),
            steering: EcuTask::new(SteeringSwc::new(config.steering_params())),
            plant_rte: Rte::default(),
            plant_alive: 0,
            bus: Bus::new(0),
            failsafe: config.failsafe_params(),
            tick: 0,
        }
    }

    /// Set or clear the external E-stop request held by the VCU task.
    pub fn set_estop_request(&mut self, on: bool) {
        self.vcu.rte.safety.estop = on;
    }

    /// Enable or stop one virtual ECU task. A stopped task sends no output or alive frame.
    pub fn set_ecu_enabled(&mut self, ecu: VirtualEcuId, enabled: bool) {
        match ecu {
            VirtualEcuId::Engine => self.engine.enabled = enabled,
            VirtualEcuId::Brake => self.brake.enabled = enabled,
            VirtualEcuId::Steering => self.steering.enabled = enabled,
        }
    }

    /// Configure the virtual vehicle bus. Production Pi tasks normally use zero delay and loss.
    pub fn configure_bus(&mut self, delay_ticks: u64, loss_permille: u32, down: bool) {
        self.bus.set_delay_ticks(delay_ticks);
        self.bus.set_loss_permille(loss_permille);
        self.bus.set_down(down);
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Execute one 10 ms cycle of virtual ECU tasks.
    ///
    /// The order matches the Rust simulation: VCU transmit, Engine/Brake/Steering tasks,
    /// platform sensor/heartbeat receive, then the VCU's 100 ms watchdog task. Driver input is
    /// transmitted every second tick. `platform_alive` supplies the fourth watchdog heartbeat.
    pub fn step_10ms(
        &mut self,
        input: DriverInput,
        vehicle_state: VehicleState,
        platform_alive: bool,
    ) -> VehicleActuation {
        let tick = self.tick;
        self.vcu.rte.input = input;
        for rte in [
            &mut self.engine.rte,
            &mut self.brake.rte,
            &mut self.steering.rte,
        ] {
            rte.rx_age.driver = rte.rx_age.driver.saturating_add(1);
            rte.rx_age.safety = rte.rx_age.safety.saturating_add(1);
        }

        if self.vcu.enabled {
            self.bus.send(tick, Frame::Safety(self.vcu.rte.safety));
            if tick % 2 == 1 {
                self.bus.send(tick, Frame::DriverInput(self.vcu.rte.input));
            }
        }
        self.deliver();

        if self.engine.enabled {
            failsafe::apply(&mut self.engine.rte, &self.failsafe);
            self.engine.swc.main_10ms(&mut self.engine.rte);
            self.engine.alive = self.engine.alive.wrapping_add(1);
            self.bus
                .send(tick, Frame::CmdDrive(self.engine.rte.cmd.drive_accel_cmd));
            self.bus.send(
                tick,
                Frame::Alive {
                    index: ALIVE_ENGINE_INDEX,
                    counter: self.engine.alive,
                },
            );
        }
        if self.brake.enabled {
            failsafe::apply(&mut self.brake.rte, &self.failsafe);
            self.brake.swc.main_10ms(&mut self.brake.rte);
            self.brake.alive = self.brake.alive.wrapping_add(1);
            self.bus
                .send(tick, Frame::CmdBrake(self.brake.rte.cmd.brake_decel_cmd));
            self.bus.send(
                tick,
                Frame::Alive {
                    index: ALIVE_BRAKE_INDEX,
                    counter: self.brake.alive,
                },
            );
        }
        if self.steering.enabled {
            failsafe::apply(&mut self.steering.rte, &self.failsafe);
            self.steering
                .swc
                .main_10ms(&mut self.steering.rte, ECU_STEP_S);
            self.steering.alive = self.steering.alive.wrapping_add(1);
            self.bus
                .send(tick, Frame::CmdSteer(self.steering.rte.cmd.steer_angle_cmd));
            self.bus.send(
                tick,
                Frame::Alive {
                    index: ALIVE_STEERING_INDEX,
                    counter: self.steering.alive,
                },
            );
        }
        self.deliver();

        self.plant_rte.state = vehicle_state;
        if platform_alive {
            self.plant_alive = self.plant_alive.wrapping_add(1);
            self.bus.send(tick, Frame::VehicleState(vehicle_state));
            self.bus.send(
                tick,
                Frame::Alive {
                    index: ALIVE_PLANT_INDEX,
                    counter: self.plant_alive,
                },
            );
        }
        self.deliver();

        if self.vcu.enabled && tick % 10 == 0 {
            self.vcu.swc.main_100ms(&mut self.vcu.rte, 100);
        }

        let output = VehicleActuation {
            command: self.plant_rte.cmd,
            safety: self.plant_rte.safety,
        };
        self.tick = self.tick.wrapping_add(1);
        output
    }

    fn deliver(&mut self) {
        for (node, frame) in self.bus.deliver(self.tick) {
            match node {
                Node::Vcu => frame.apply(&mut self.vcu.rte),
                Node::Engine => frame.apply(&mut self.engine.rte),
                Node::Brake => frame.apply(&mut self.brake.rte),
                Node::Steering => frame.apply(&mut self.steering.rte),
                Node::Plant => frame.apply(&mut self.plant_rte),
            }
        }
    }
}

impl Default for VehicleRuntime {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{VehicleRuntime, VirtualEcuId};
    use crate::rte::DriverInput;
    use crate::sim::{Sim, SystemState};

    #[test]
    fn platform_runtime_matches_virtual_simulation_actuation() {
        let mut runtime = VehicleRuntime::default();
        let mut sim = Sim::new();
        let input = DriverInput {
            throttle: 0.5,
            brake: 0.25,
            steer: 0.1,
        };

        for _ in 0..200 {
            sim.input_mut().throttle = input.throttle;
            sim.input_mut().brake = input.brake;
            sim.input_mut().steer = input.steer;
            let expected = sim.step_10ms();
            let actual = runtime.step_10ms(input, expected.state, true);
            assert_eq!(actual.command, expected.cmd);
            assert_eq!(actual.safety, expected.safety);
        }
    }

    #[test]
    fn one_stopped_virtual_ecu_does_not_stop_the_other_tasks() {
        let mut runtime = VehicleRuntime::default();
        runtime.set_ecu_enabled(VirtualEcuId::Engine, false);
        let input = DriverInput {
            throttle: 0.5,
            brake: 0.25,
            steer: 0.1,
        };

        for _ in 0..120 {
            runtime.step_10ms(input, crate::model::VehicleState::default(), true);
        }

        let output = runtime.step_10ms(input, crate::model::VehicleState::default(), true);
        assert_eq!(output.command.drive_accel_cmd, 0.0);
        assert!(output.command.brake_decel_cmd > 0.0);
        assert_ne!(output.command.steer_angle_cmd, 0.0);
        assert_eq!(output.safety.system_state, SystemState::Degraded);
    }
}
