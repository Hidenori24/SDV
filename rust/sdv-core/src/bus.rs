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
            Frame::DriverInput(v) => rte.input = v,
            Frame::Safety(v) => rte.safety = v,
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

#[derive(Debug, Default)]
pub struct Bus {
    delay_ticks: u64,
    inflight: Vec<InFlight>,
}

impl Bus {
    pub fn new(delay_ticks: u64) -> Self {
        Self { delay_ticks, inflight: Vec::new() }
    }

    /// Queue a frame for all of its receivers.
    pub fn send(&mut self, now_tick: u64, frame: Frame) {
        for &to in frame.receivers() {
            self.inflight.push(InFlight { due_tick: now_tick + self.delay_ticks, to, frame });
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
