//! Virtual bus between ECUs (CAN-like: fixed-period frames, latest value wins).
//!
//! A frame is sent by one node and delivered to its receivers after `delay_ticks`
//! (10 ms ticks). With zero delay a frame sent in a tick is delivered in the same tick,
//! at the next `deliver()` call.

use crate::model::VehicleState;
use crate::rte::{DriverInput, Rte, Safety};

/// A participant on the bus. `Plant` is the physical world (not an ECU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    Vcu,
    Engine,
    Brake,
    Steering,
    Plant,
}

/// Index of a supervised node in the alive-counter array (same order as `sim::Component`).
pub const ALIVE_NODES: [Node; 4] = [Node::Engine, Node::Brake, Node::Steering, Node::Plant];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// VCU -> Engine / Brake / Steering, 20 ms
    DriverInput(DriverInput),
    /// VCU -> all, 10 ms
    Safety(Safety),
    /// supervised node -> VCU, 10 ms. `index` = position in `ALIVE_NODES`
    Alive { index: usize, counter: u64 },
    /// Plant -> VCU, 10 ms (sensor values)
    VehicleState(VehicleState),
    /// Engine -> Plant, 10 ms
    CmdDrive(f32),
    /// Brake -> Plant, 10 ms
    CmdBrake(f32),
    /// Steering -> Plant, 10 ms
    CmdSteer(f32),
}

impl Frame {
    /// Nodes that receive this frame (the bus matrix of ADR-0002).
    pub fn receivers(&self) -> &'static [Node] {
        match self {
            Frame::DriverInput(_) => &[Node::Engine, Node::Brake, Node::Steering],
            Frame::Safety(_) => &[Node::Engine, Node::Brake, Node::Steering, Node::Plant],
            Frame::Alive { .. } | Frame::VehicleState(_) => &[Node::Vcu],
            Frame::CmdDrive(_) | Frame::CmdBrake(_) | Frame::CmdSteer(_) => &[Node::Plant],
        }
    }

    /// Write the received value into the receiver's RTE.
    pub fn apply(&self, rte: &mut Rte) {
        match *self {
            Frame::DriverInput(v) => {
                rte.input = v;
                rte.rx_age.driver = 0;
            }
            Frame::Safety(v) => {
                rte.safety = v;
                rte.rx_age.safety = 0;
            }
            Frame::Alive { index, counter } => rte.alive_rx[index] = counter,
            Frame::VehicleState(v) => rte.state = v,
            Frame::CmdDrive(v) => rte.cmd.drive_accel_cmd = v,
            Frame::CmdBrake(v) => rte.cmd.brake_decel_cmd = v,
            Frame::CmdSteer(v) => rte.cmd.steer_angle_cmd = v,
        }
    }
}

#[derive(Debug)]
struct InFlight {
    due_tick: u64,
    to: Node,
    frame: Frame,
}

#[derive(Debug)]
pub struct Bus {
    delay_ticks: u64,
    loss_permille: u32,
    down: bool,
    /// Deterministic generator for frame loss, so a run is reproducible.
    rng: u32,
    inflight: Vec<InFlight>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Bus {
    pub fn new(delay_ticks: u64) -> Self {
        Self { delay_ticks, loss_permille: 0, down: false, rng: 0x1234_5678, inflight: Vec::new() }
    }

    /// Delay of the vehicle bus in ticks (10 ms) for frames sent from now on.
    pub fn set_delay_ticks(&mut self, ticks: u64) {
        self.delay_ticks = ticks;
    }

    /// Probability (per mille) that a frame is lost on the vehicle bus.
    pub fn set_loss_permille(&mut self, permille: u32) {
        self.loss_permille = permille.min(1000);
    }

    /// Bus down: every frame on the vehicle bus is lost.
    pub fn set_down(&mut self, down: bool) {
        self.down = down;
    }

    pub fn is_down(&self) -> bool {
        self.down
    }

    fn next_rand(&mut self) -> u32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// Queue a frame for all of its receivers.
    ///
    /// Links into the plant are direct wiring (actuators, physical E-Stop line): delay, loss and
    /// bus-down only apply to the vehicle bus (every other receiver).
    pub fn send(&mut self, now_tick: u64, frame: Frame) {
        for &to in frame.receivers() {
            let direct = to == Node::Plant;
            if !direct {
                if self.down {
                    continue;
                }
                if self.loss_permille > 0 && self.next_rand() % 1000 < self.loss_permille {
                    continue;
                }
            }
            let delay = if direct { 0 } else { self.delay_ticks };
            self.inflight.push(InFlight { due_tick: now_tick + delay, to, frame });
        }
    }

    /// Remove and return every frame that is due at `now_tick`, in send order.
    pub fn deliver(&mut self, now_tick: u64) -> Vec<(Node, Frame)> {
        let mut due = Vec::new();
        self.inflight.retain(|f| {
            if f.due_tick <= now_tick {
                due.push((f.to, f.frame));
                false
            } else {
                true
            }
        });
        due
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }
}
