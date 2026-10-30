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

/// One independent, platform-neutral Engine, Brake, or Steering task.
///
/// The task owns its RTE and SWC state. Call `step_10ms` once per 10 ms cycle with the frames
/// delivered to it by a platform bus adapter. Its output contains the task's actuator command and
/// alive frame; the adapter decides how to transmit those frames.
pub struct VirtualEcuTask {
    id: VirtualEcuId,
    rte: Rte,
    swc: EcuSwc,
    alive: u64,
    enabled: bool,
    failsafe: FailsafeParams,
}

enum EcuSwc {
    Engine(EngineSwc),
    Brake(BrakeSwc),
    Steering(SteeringSwc),
}

/// The two bus frames emitted by one virtual ECU task cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcuTaskOutput {
    pub command: Frame,
    pub alive: Frame,
}

/// Failure when a host dispatches ECU cycles to an external backend.
#[derive(Debug, PartialEq, Eq)]
pub enum ExternalEcuCycleError<E> {
    /// External dispatch is unsupported while any local ECU task is disabled.
    DisabledTask(VirtualEcuId),
    /// The external dispatcher failed after the cycle began. The runtime is then poisoned and
    /// must be discarded because a subset of workers may already have advanced.
    Dispatch(E),
    /// A previous external cycle failed; this runtime cannot safely be reused.
    Poisoned,
}

impl VirtualEcuId {
    fn node(self) -> Node {
        match self {
            Self::Engine => Node::Engine,
            Self::Brake => Node::Brake,
            Self::Steering => Node::Steering,
        }
    }

    fn alive_index(self) -> usize {
        match self {
            Self::Engine => ALIVE_ENGINE_INDEX,
            Self::Brake => ALIVE_BRAKE_INDEX,
            Self::Steering => ALIVE_STEERING_INDEX,
        }
    }
}

impl VirtualEcuTask {
    /// Create one virtual ECU task with its ECU-specific and shared failsafe parameters.
    pub fn new(id: VirtualEcuId, config: &Config) -> Self {
        let swc = match id {
            VirtualEcuId::Engine => EcuSwc::Engine(EngineSwc::new(config.engine_params())),
            VirtualEcuId::Brake => EcuSwc::Brake(BrakeSwc::new(config.brake_params())),
            VirtualEcuId::Steering => EcuSwc::Steering(SteeringSwc::new(config.steering_params())),
        };
        Self {
            id,
            rte: Rte::default(),
            swc,
            alive: 0,
            enabled: true,
            failsafe: config.failsafe_params(),
        }
    }

    pub fn id(&self) -> VirtualEcuId {
        self.id
    }

    /// Enable or stop this task. A stopped task still receives frames, but emits no output.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Advance one 10 ms task cycle and return its actuator and alive frames.
    ///
    /// Receive ages advance before frames are applied, matching the in-process vehicle bus cycle.
    /// Frames not addressed to this ECU are ignored. A disabled task keeps receiving frames but
    /// emits neither a command nor an alive heartbeat.
    pub fn step_10ms(&mut self, incoming: &[Frame]) -> Option<EcuTaskOutput> {
        self.rte.rx_age.driver = self.rte.rx_age.driver.saturating_add(1);
        self.rte.rx_age.safety = self.rte.rx_age.safety.saturating_add(1);
        let node = self.id.node();
        for frame in incoming {
            if frame.receivers().contains(&node) {
                frame.apply(&mut self.rte);
            }
        }

        if !self.enabled {
            return None;
        }

        failsafe::apply(&mut self.rte, &self.failsafe);
        let command = match &mut self.swc {
            EcuSwc::Engine(swc) => {
                swc.main_10ms(&mut self.rte);
                Frame::CmdDrive(self.rte.cmd.drive_accel_cmd)
            }
            EcuSwc::Brake(swc) => {
                swc.main_10ms(&mut self.rte);
                Frame::CmdBrake(self.rte.cmd.brake_decel_cmd)
            }
            EcuSwc::Steering(swc) => {
                swc.main_10ms(&mut self.rte, ECU_STEP_S);
                Frame::CmdSteer(self.rte.cmd.steer_angle_cmd)
            }
        };
        self.alive = self.alive.wrapping_add(1);
        Some(EcuTaskOutput {
            command,
            alive: Frame::Alive {
                index: self.id.alive_index(),
                counter: self.alive,
            },
        })
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
    engine: VirtualEcuTask,
    brake: VirtualEcuTask,
    steering: VirtualEcuTask,
    plant_rte: Rte,
    plant_alive: u64,
    bus: Bus,
    tick: u64,
    external_cycle_poisoned: bool,
}

impl VehicleRuntime {
    pub fn new(config: Config) -> Self {
        Self {
            vcu: VcuTask {
                rte: Rte::default(),
                swc: SafetySwc::new(config.safety_params()),
                enabled: true,
            },
            engine: VirtualEcuTask::new(VirtualEcuId::Engine, &config),
            brake: VirtualEcuTask::new(VirtualEcuId::Brake, &config),
            steering: VirtualEcuTask::new(VirtualEcuId::Steering, &config),
            plant_rte: Rte::default(),
            plant_alive: 0,
            bus: Bus::new(0),
            tick: 0,
            external_cycle_poisoned: false,
        }
    }

    /// Set or clear the external E-stop request held by the VCU task.
    pub fn set_estop_request(&mut self, on: bool) {
        self.vcu.rte.safety.estop = on;
    }

    /// Enable or stop one virtual ECU task. A stopped task sends no output or alive frame.
    pub fn set_ecu_enabled(&mut self, ecu: VirtualEcuId, enabled: bool) {
        match ecu {
            VirtualEcuId::Engine => self.engine.set_enabled(enabled),
            VirtualEcuId::Brake => self.brake.set_enabled(enabled),
            VirtualEcuId::Steering => self.steering.set_enabled(enabled),
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
        let incoming = self.begin_cycle(input);
        let outputs = [
            self.engine.step_10ms(&incoming[0]),
            self.brake.step_10ms(&incoming[1]),
            self.steering.step_10ms(&incoming[2]),
        ];
        self.finish_cycle(tick, outputs, vehicle_state, platform_alive)
    }

    /// Execute one cycle while Engine, Brake, and Steering are dispatched by an external
    /// coordinator such as the Linux Unix-datagram worker adapter.
    ///
    /// This method owns the same VCU transmit, receive-age, bus routing, vehicle-state, watchdog,
    /// and tick sequencing as [`Self::step_10ms`]. The dispatcher receives all three sets of frames
    /// delivered by the in-process bus and must return one command/alive output per ECU. A batch
    /// callback lets a platform send every worker request before it waits for any response. If
    /// dispatch fails after any worker has advanced, the runtime must be
    /// abandoned: a caller must stop the platform and must not retry this cycle with the same
    /// runtime or workers.
    pub fn step_10ms_with_external_ecus<E>(
        &mut self,
        input: DriverInput,
        vehicle_state: VehicleState,
        platform_alive: bool,
        mut dispatch: impl FnMut(u64, [&[Frame]; 3]) -> Result<[EcuTaskOutput; 3], E>,
    ) -> Result<VehicleActuation, ExternalEcuCycleError<E>> {
        if self.external_cycle_poisoned {
            return Err(ExternalEcuCycleError::Poisoned);
        }
        for (id, enabled) in [
            (VirtualEcuId::Engine, self.engine.enabled),
            (VirtualEcuId::Brake, self.brake.enabled),
            (VirtualEcuId::Steering, self.steering.enabled),
        ] {
            if !enabled {
                return Err(ExternalEcuCycleError::DisabledTask(id));
            }
        }
        let tick = self.tick;
        let incoming = self.begin_cycle(input);
        let outputs = match dispatch(
            tick,
            [&incoming[0], &incoming[1], &incoming[2]].map(Vec::as_slice),
        ) {
            Ok(outputs) => outputs.map(Some),
            Err(error) => {
                self.external_cycle_poisoned = true;
                return Err(ExternalEcuCycleError::Dispatch(error));
            }
        };
        Ok(self.finish_cycle(tick, outputs, vehicle_state, platform_alive))
    }

    fn begin_cycle(&mut self, input: DriverInput) -> [Vec<Frame>; 3] {
        let tick = self.tick;
        self.vcu.rte.input = input;
        if self.vcu.enabled {
            self.bus.send(tick, Frame::Safety(self.vcu.rte.safety));
            if tick % 2 == 1 {
                self.bus.send(tick, Frame::DriverInput(self.vcu.rte.input));
            }
        }
        self.deliver_to_ecus()
    }

    fn finish_cycle(
        &mut self,
        tick: u64,
        outputs: [Option<EcuTaskOutput>; 3],
        vehicle_state: VehicleState,
        platform_alive: bool,
    ) -> VehicleActuation {
        for output in outputs.into_iter().flatten() {
            self.bus.send(tick, output.command);
            self.bus.send(tick, output.alive);
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

    fn deliver_to_ecus(&mut self) -> [Vec<Frame>; 3] {
        let mut incoming: [Vec<Frame>; 3] = std::array::from_fn(|_| Vec::new());
        for (node, frame) in self.bus.deliver(self.tick) {
            match node {
                Node::Vcu => frame.apply(&mut self.vcu.rte),
                Node::Engine => incoming[0].push(frame),
                Node::Brake => incoming[1].push(frame),
                Node::Steering => incoming[2].push(frame),
                Node::Plant => frame.apply(&mut self.plant_rte),
            }
        }
        incoming
    }

    fn deliver(&mut self) {
        for (node, frame) in self.bus.deliver(self.tick) {
            match node {
                Node::Vcu => frame.apply(&mut self.vcu.rte),
                Node::Engine | Node::Brake | Node::Steering => {
                    // The vehicle runtime routes ECU input frames before stepping the tasks.
                    // ECU output frames are addressed only to the VCU or Plant.
                }
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
    use super::{VehicleRuntime, VirtualEcuId, ECU_STEP_S};
    use crate::rte::DriverInput;
    use crate::sim::{Sim, SystemState};

    #[test]
    fn individual_ecu_tasks_exchange_their_own_command_and_alive_frames() {
        use super::VirtualEcuTask;
        use crate::bus::{Frame, ALIVE_BRAKE_INDEX, ALIVE_ENGINE_INDEX, ALIVE_STEERING_INDEX};
        use crate::config::Config;
        use crate::rte::{DriverInput, Safety};

        let input = [
            Frame::Safety(Safety::default()),
            Frame::DriverInput(DriverInput {
                throttle: 0.5,
                brake: 0.25,
                steer: 0.1,
            }),
        ];
        for (ecu, alive_index) in [
            (VirtualEcuId::Engine, ALIVE_ENGINE_INDEX),
            (VirtualEcuId::Brake, ALIVE_BRAKE_INDEX),
            (VirtualEcuId::Steering, ALIVE_STEERING_INDEX),
        ] {
            let mut task = VirtualEcuTask::new(ecu, &Config::default());
            let first = task.step_10ms(&input).expect("enabled ECU emits one cycle");
            assert!(match ecu {
                VirtualEcuId::Engine => matches!(first.command, Frame::CmdDrive(_)),
                VirtualEcuId::Brake => matches!(first.command, Frame::CmdBrake(_)),
                VirtualEcuId::Steering => matches!(first.command, Frame::CmdSteer(_)),
            });
            assert_eq!(
                first.alive,
                Frame::Alive {
                    index: alive_index,
                    counter: 1,
                }
            );

            let second = task.step_10ms(&[]).expect("enabled ECU emits next cycle");
            assert!(match ecu {
                VirtualEcuId::Engine => matches!(second.command, Frame::CmdDrive(_)),
                VirtualEcuId::Brake => matches!(second.command, Frame::CmdBrake(_)),
                VirtualEcuId::Steering => matches!(second.command, Frame::CmdSteer(_)),
            });
            assert_eq!(
                second.alive,
                Frame::Alive {
                    index: alive_index,
                    counter: 2,
                }
            );
        }
    }

    #[test]
    fn disabled_individual_task_receives_frames_but_emits_no_frames() {
        use super::VirtualEcuTask;
        use crate::bus::{Frame, ALIVE_ENGINE_INDEX};
        use crate::config::Config;
        use crate::rte::Safety;

        let mut task = VirtualEcuTask::new(VirtualEcuId::Engine, &Config::default());
        task.set_enabled(false);
        assert!(task
            .step_10ms(&[Frame::Safety(Safety::default())])
            .is_none());

        task.set_enabled(true);
        let output = task.step_10ms(&[]).expect("re-enabled ECU executes");
        assert_eq!(
            output.alive,
            Frame::Alive {
                index: ALIVE_ENGINE_INDEX,
                counter: 1,
            }
        );
    }

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
    fn external_ecu_dispatch_matches_the_in_process_cycle_boundary() {
        use super::{EcuTaskOutput, VirtualEcuTask};
        use crate::config::Config;
        use crate::model::VehicleState;

        let config = Config::default();
        let mut local = VehicleRuntime::new(config.clone());
        let mut dispatched = VehicleRuntime::new(config.clone());
        let mut tasks = [
            VirtualEcuTask::new(VirtualEcuId::Engine, &config),
            VirtualEcuTask::new(VirtualEcuId::Brake, &config),
            VirtualEcuTask::new(VirtualEcuId::Steering, &config),
        ];

        for cycle in 0..250_u64 {
            let input = DriverInput {
                throttle: if cycle % 60 < 30 { 0.4 } else { 0.0 },
                brake: if cycle % 60 < 30 { 0.0 } else { 0.2 },
                steer: if cycle % 40 < 20 { 0.1 } else { -0.1 },
            };
            let mut state = VehicleState::default();
            state.t = cycle as f32 * ECU_STEP_S;
            state.v = input.throttle - input.brake;
            local.set_estop_request(cycle == 180);
            dispatched.set_estop_request(cycle == 180);

            let expected = local.step_10ms(input, state, true);
            let actual = dispatched
                .step_10ms_with_external_ecus(input, state, true, |cycle_id, frames| {
                    assert_eq!(cycle_id, cycle);
                    let engine: EcuTaskOutput = tasks[0]
                        .step_10ms(frames[0])
                        .expect("enabled Engine task emits one response");
                    let brake: EcuTaskOutput = tasks[1]
                        .step_10ms(frames[1])
                        .expect("enabled Brake task emits one response");
                    let steering: EcuTaskOutput = tasks[2]
                        .step_10ms(frames[2])
                        .expect("enabled Steering task emits one response");
                    Ok::<_, ()>([engine, brake, steering])
                })
                .unwrap();
            assert_eq!(actual, expected, "actuation diverged at cycle {cycle}");
        }
    }

    #[test]
    fn external_dispatch_rejects_disabled_tasks_and_poisons_after_worker_failure() {
        use super::ExternalEcuCycleError;
        use crate::model::VehicleState;

        let mut disabled = VehicleRuntime::default();
        disabled.set_ecu_enabled(VirtualEcuId::Engine, false);
        let mut dispatched = false;
        let result = disabled.step_10ms_with_external_ecus(
            DriverInput::default(),
            VehicleState::default(),
            true,
            |_, _| -> Result<[super::EcuTaskOutput; 3], &'static str> {
                dispatched = true;
                unreachable!("disabled tasks must be rejected before contacting workers")
            },
        );
        assert_eq!(
            result,
            Err(ExternalEcuCycleError::DisabledTask(VirtualEcuId::Engine))
        );
        assert!(!dispatched);
        assert_eq!(disabled.tick(), 0);

        let mut failed = VehicleRuntime::default();
        let result = failed.step_10ms_with_external_ecus(
            DriverInput::default(),
            VehicleState::default(),
            true,
            |_, _| Err::<[super::EcuTaskOutput; 3], _>("worker failure"),
        );
        assert_eq!(
            result,
            Err(ExternalEcuCycleError::Dispatch("worker failure"))
        );
        let result = failed.step_10ms_with_external_ecus(
            DriverInput::default(),
            VehicleState::default(),
            true,
            |_, _| -> Result<[super::EcuTaskOutput; 3], &'static str> {
                unreachable!("a poisoned runtime must not dispatch another cycle")
            },
        );
        assert_eq!(result, Err(ExternalEcuCycleError::Poisoned));
        assert_eq!(failed.tick(), 0);
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
