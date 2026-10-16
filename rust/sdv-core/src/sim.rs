//! Fixed-step scheduler + minimal RTE, mirroring the C++ `main.cpp` task order:
//! 10 ms: Engine -> Brake -> Steering -> VehicleDynamics -> log,
//! then 20 ms: DriverInput, 100 ms: Safety.

use crate::bus::{Bus, Frame, Node};
use crate::config::Config;
use crate::scenario::{Action, Event, Scenario, Target};
use crate::failsafe::{self, FailsafeParams, FailsafeStatus};
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

/// An ECU = its own RTE + the SWCs allocated to it + alive counter + fault-injection flag.
struct Ecu<S> {
    rte: Rte,
    swc: S,
    alive: u64,
    stalled: bool,
}

impl<S> Ecu<S> {
    fn with_swc(swc: S) -> Self {
        Self { rte: Rte::default(), swc, alive: 0, stalled: false }
    }
}

/// VCU / gateway ECU: DriverInput + Safety (watchdog).
struct Vcu {
    rte: Rte,
    safety_swc: SafetySwc,
    stalled: bool,
}

/// Fixed ECU layout of ADR-0002 plan A:
/// VCU, Engine ECU, Brake ECU, Steering ECU, and the Plant (VehicleDynamics), joined by a virtual bus.
pub struct Sim {
    config: Config,
    vcu: Vcu,
    engine: Ecu<EngineSwc>,
    brake: Ecu<BrakeSwc>,
    steering: Ecu<SteeringSwc>,
    plant: Ecu<VehicleDynamicsSwc>,
    bus: Bus,
    failsafe: FailsafeParams,
    /// true: DriverInput follows the built-in demo scenario (like the C++ app);
    /// false: the input is controlled from outside (GUI).
    use_scenario: bool,
    scenario: Scenario,
    /// Scenario events sorted by time, and how many have fired already.
    events: Vec<Event>,
    next_event: usize,
    tick: u64,
}

impl Sim {
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    /// Boot with a given (already validated) config.
    pub fn with_config(config: Config) -> Self {
        Self {
            vcu: Vcu { rte: Rte::default(), safety_swc: SafetySwc::new(config.safety_params()), stalled: false },
            engine: Ecu::with_swc(EngineSwc::new(config.engine_params())),
            brake: Ecu::with_swc(BrakeSwc::new(config.brake_params())),
            steering: Ecu::with_swc(SteeringSwc::new(config.steering_params())),
            plant: Ecu::with_swc(VehicleDynamicsSwc::new(config.vehicle_params())),
            bus: Bus::new(0),
            failsafe: config.failsafe_params(),
            config,
            use_scenario: false,
            scenario: Scenario::demo(),
            events: Vec::new(),
            next_event: 0,
            tick: 0,
        }
    }

    pub fn config(&self) -> &Config { &self.config }

    /// Install a new config and restart (a config update takes effect on reboot, like on a real ECU).
    /// The demo-scenario flag is kept; injected faults are cleared.
    pub fn set_config(&mut self, config: Config) {
        let use_scenario = self.use_scenario;
        let scenario = std::mem::take(&mut self.scenario);
        *self = Self::with_config(config);
        self.use_scenario = use_scenario;
        self.set_scenario(scenario);
    }

    /// Install the scenario used in scenario mode (inputs + events). Events restart from the beginning.
    pub fn set_scenario(&mut self, scenario: Scenario) {
        self.events = scenario.sorted_events();
        self.scenario = scenario;
        self.next_event = 0;
    }

    pub fn scenario(&self) -> &Scenario { &self.scenario }

    fn apply_event(&mut self, ev: Event) {
        let comp = |t: Option<Target>| match t {
            Some(Target::Engine) => Some(Component::Engine),
            Some(Target::Brake) => Some(Component::Brake),
            Some(Target::Steering) => Some(Component::Steering),
            Some(Target::Vehicledynamics) => Some(Component::VehicleDynamics),
            None => None,
        };
        match ev.action {
            Action::Stall | Action::Resume => {
                if let Some(c) = comp(ev.target) {
                    self.set_stall(c, ev.action == Action::Stall);
                }
            }
            Action::Estop => self.set_estop_request(true),
            Action::EstopRelease => self.set_estop_request(false),
            Action::BusDown => self.set_bus_down(true),
            Action::BusUp => self.set_bus_down(false),
            Action::VcuStall => self.set_vcu_stall(true),
            Action::VcuResume => self.set_vcu_stall(false),
        }
    }

    // ---- signal access (the GUI / tests see the system through these) ----
    pub fn input(&self) -> DriverInput { self.vcu.rte.input }
    /// The driver's request lives on the VCU; it reaches the ECUs through the bus.
    pub fn input_mut(&mut self) -> &mut DriverInput { &mut self.vcu.rte.input }
    /// Actuator commands as received by the plant.
    pub fn cmd(&self) -> ActuatorCmd { self.plant.rte.cmd }
    pub fn state(&self) -> VehicleState { self.plant.rte.state }
    pub fn safety(&self) -> Safety { self.vcu.rte.safety }
    pub fn set_use_scenario(&mut self, on: bool) { self.use_scenario = on; }

    /// RTE of a node (for inspection).
    pub fn rte(&self, node: Node) -> &Rte {
        match node {
            Node::Vcu => &self.vcu.rte,
            Node::Engine => &self.engine.rte,
            Node::Brake => &self.brake.rte,
            Node::Steering => &self.steering.rte,
            Node::Plant => &self.plant.rte,
        }
    }

    fn rte_mut(&mut self, node: Node) -> &mut Rte {
        match node {
            Node::Vcu => &mut self.vcu.rte,
            Node::Engine => &mut self.engine.rte,
            Node::Brake => &mut self.brake.rte,
            Node::Steering => &mut self.steering.rte,
            Node::Plant => &mut self.plant.rte,
        }
    }

    /// Fault injection (like `sdv_sim --stall`): stop (`true`) or resume (`false`) a node.
    /// A stalled node runs nothing and sends no frames (so its alive counter stops too).
    pub fn set_stall(&mut self, c: Component, stalled: bool) {
        match c {
            Component::Engine => self.engine.stalled = stalled,
            Component::Brake => self.brake.stalled = stalled,
            Component::Steering => self.steering.stalled = stalled,
            Component::VehicleDynamics => self.plant.stalled = stalled,
        }
    }

    pub fn is_stalled(&self, c: Component) -> bool {
        match c {
            Component::Engine => self.engine.stalled,
            Component::Brake => self.brake.stalled,
            Component::Steering => self.steering.stalled,
            Component::VehicleDynamics => self.plant.stalled,
        }
    }

    pub fn vcu_stalled(&self) -> bool { self.vcu.stalled }
    pub fn bus_is_down(&self) -> bool { self.bus.is_down() }

    /// Stall the VCU (DriverInput + Safety): it neither runs nor sends frames.
    pub fn set_vcu_stall(&mut self, stalled: bool) {
        self.vcu.stalled = stalled;
    }

    /// Vehicle-bus faults (they do not affect the direct ECU -> plant links).
    pub fn set_bus_delay_ticks(&mut self, ticks: u64) { self.bus.set_delay_ticks(ticks); }
    pub fn set_bus_loss_permille(&mut self, permille: u32) { self.bus.set_loss_permille(permille); }
    pub fn set_bus_down(&mut self, down: bool) { self.bus.set_down(down); }

    /// Receiving-side fail-safe state of an ECU (always clear for the VCU and the plant).
    pub fn ecu_failsafe(&self, node: Node) -> FailsafeStatus {
        match node {
            Node::Engine | Node::Brake | Node::Steering => failsafe::status(self.rte(node), &self.failsafe),
            Node::Vcu | Node::Plant => FailsafeStatus::default(),
        }
    }

    /// External E-Stop request (UI / hardware); enters on the VCU and reaches the ECUs via the Safety frame.
    pub fn set_estop_request(&mut self, on: bool) {
        self.vcu.rte.safety.estop = on;
    }

    /// Watchdog view of each supervised node: [engine, brake, steering, vehicledynamics].
    pub fn swc_status(&self) -> [SwcStatus; 4] {
        self.vcu.safety_swc.supervisor.status()
    }

    /// Reset everything (this releases a latched watchdog EStop and clears injected stalls).
    pub fn reset(&mut self) {
        let config = self.config.clone();
        self.set_config(config);
    }

    fn deliver(&mut self) {
        for (to, frame) in self.bus.deliver(self.tick) {
            frame.apply(self.rte_mut(to));
        }
    }

    /// Advance one 10 ms tick and return the log row.
    ///
    /// Order: VCU transmits -> ECUs run and transmit -> plant runs and transmits -> VCU tasks.
    /// The bus delivers between the phases, so with zero delay every value is seen in the same
    /// tick as in the single-process version.
    pub fn step_10ms(&mut self) -> Row {
        let i = self.tick;
        let dt = DT_10MS;

        // Scenario events fire when the simulated time (the plant's clock) reaches them.
        if self.use_scenario {
            while self.next_event < self.events.len() && self.plant.rte.state.t >= self.events[self.next_event].at_s {
                let ev = self.events[self.next_event];
                self.next_event += 1;
                self.apply_event(ev);
            }
        }

        // Phase 0: VCU transmits (Safety every 10 ms, DriverInput every 20 ms)
        for ecu_rte in [&mut self.engine.rte, &mut self.brake.rte, &mut self.steering.rte] {
            ecu_rte.rx_age.driver = ecu_rte.rx_age.driver.saturating_add(1);
            ecu_rte.rx_age.safety = ecu_rte.rx_age.safety.saturating_add(1);
        }
        if !self.vcu.stalled {
            self.bus.send(i, Frame::Safety(self.vcu.rte.safety));
            if i % 2 == 1 {
                self.bus.send(i, Frame::DriverInput(self.vcu.rte.input));
            }
        }
        self.deliver();

        // Phase 1: ECUs
        if !self.engine.stalled {
            failsafe::apply(&mut self.engine.rte, &self.failsafe);
            self.engine.swc.main_10ms(&mut self.engine.rte);
            self.engine.alive += 1;
            self.bus.send(i, Frame::CmdDrive(self.engine.rte.cmd.drive_accel_cmd));
            self.bus.send(i, Frame::Alive { index: Component::Engine as usize, counter: self.engine.alive });
        }
        if !self.brake.stalled {
            failsafe::apply(&mut self.brake.rte, &self.failsafe);
            self.brake.swc.main_10ms(&mut self.brake.rte);
            self.brake.alive += 1;
            self.bus.send(i, Frame::CmdBrake(self.brake.rte.cmd.brake_decel_cmd));
            self.bus.send(i, Frame::Alive { index: Component::Brake as usize, counter: self.brake.alive });
        }
        if !self.steering.stalled {
            failsafe::apply(&mut self.steering.rte, &self.failsafe);
            self.steering.swc.main_10ms(&mut self.steering.rte, dt);
            self.steering.alive += 1;
            self.bus.send(i, Frame::CmdSteer(self.steering.rte.cmd.steer_angle_cmd));
            self.bus.send(i, Frame::Alive { index: Component::Steering as usize, counter: self.steering.alive });
        }
        self.deliver();

        // Phase 2: plant (a stalled plant also freezes simulated time, as in C++)
        if !self.plant.stalled {
            self.plant.swc.step_10ms(&mut self.plant.rte, dt);
            self.plant.alive += 1;
            self.bus.send(i, Frame::VehicleState(self.plant.rte.state));
            self.bus.send(i, Frame::Alive { index: Component::VehicleDynamics as usize, counter: self.plant.alive });
        }

        let row = Row {
            input: self.vcu.rte.input,
            cmd: self.plant.rte.cmd,
            state: self.plant.rte.state,
            safety: self.vcu.rte.safety,
        };
        self.deliver();

        // Phase 3: VCU tasks (see the sensor values and alive counters of this tick)
        if !self.vcu.stalled {
            if i % 2 == 0 && self.use_scenario {
                DriverInputSwc::main_20ms_scenario(&mut self.vcu.rte, &self.scenario);
            }
            if i % 10 == 0 {
                self.vcu.safety_swc.main_100ms(&mut self.vcu.rte, DT_100MS_MS);
            }
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
