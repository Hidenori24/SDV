//! Virtual bus between ECUs (CAN-like: fixed-period frames, latest value wins).
//!
//! A frame is sent by one node and delivered to its receivers after `delay_ticks`
//! (10 ms ticks). With zero delay a frame sent in a tick is delivered in the same tick,
//! at the next `deliver()` call.

use crate::model::VehicleState;
use crate::rte::{DriverInput, Rte, Safety, SystemState};

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
pub const ALIVE_ENGINE_INDEX: usize = 0;
pub const ALIVE_BRAKE_INDEX: usize = 1;
pub const ALIVE_STEERING_INDEX: usize = 2;
pub const ALIVE_PLANT_INDEX: usize = 3;
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

/// Versioned wire format for transferring one bus frame without allocation.
pub const FRAME_WIRE_VERSION: u8 = 1;
/// Capacity required by `Frame::encode_wire`: header + largest payload + CRC32.
pub const FRAME_WIRE_MAX_SIZE: usize = 39;
const FRAME_WIRE_MAGIC: [u8; 4] = *b"SDVF";
const FRAME_WIRE_HEADER_SIZE: usize = 7;
const FRAME_WIRE_CRC_SIZE: usize = 4;

const FRAME_KIND_DRIVER_INPUT: u8 = 1;
const FRAME_KIND_SAFETY: u8 = 2;
const FRAME_KIND_ALIVE: u8 = 3;
const FRAME_KIND_VEHICLE_STATE: u8 = 4;
const FRAME_KIND_CMD_DRIVE: u8 = 5;
const FRAME_KIND_CMD_BRAKE: u8 = 6;
const FRAME_KIND_CMD_STEER: u8 = 7;

const DRIVER_INPUT_PAYLOAD_SIZE: usize = 12;
const SAFETY_PAYLOAD_SIZE: usize = 2;
const ALIVE_PAYLOAD_SIZE: usize = 9;
const VEHICLE_STATE_PAYLOAD_SIZE: usize = 28;
const COMMAND_PAYLOAD_SIZE: usize = 4;

/// Errors returned when a frame cannot be encoded or a wire frame is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWireError {
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion,
    UnknownKind,
    ChecksumMismatch,
    InvalidEnum,
    InvalidAliveIndex,
    NonFiniteValue,
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

    /// Encode this frame into a fixed-size buffer and return its used byte length.
    ///
    /// The wire record is `SDVF | version | kind | payload length | payload | CRC32`, with
    /// multibyte values encoded little-endian. CRC32 detects accidental corruption; it is not
    /// authentication. The output buffer is cleared before writing.
    pub fn encode_wire(
        &self,
        output: &mut [u8; FRAME_WIRE_MAX_SIZE],
    ) -> Result<usize, FrameWireError> {
        match *self {
            Frame::DriverInput(input)
                if ![input.throttle, input.brake, input.steer]
                    .iter()
                    .all(|value| value.is_finite()) =>
            {
                return Err(FrameWireError::NonFiniteValue);
            }
            Frame::VehicleState(state)
                if ![
                    state.t,
                    state.x,
                    state.y,
                    state.yaw,
                    state.v,
                    state.yaw_rate,
                    state.wheel_omega,
                ]
                .iter()
                .all(|value| value.is_finite()) =>
            {
                return Err(FrameWireError::NonFiniteValue);
            }
            Frame::CmdDrive(value) | Frame::CmdBrake(value) | Frame::CmdSteer(value)
                if !value.is_finite() =>
            {
                return Err(FrameWireError::NonFiniteValue);
            }
            Frame::Alive { index, .. } if index >= ALIVE_NODES.len() => {
                return Err(FrameWireError::InvalidAliveIndex);
            }
            _ => {}
        }

        let (kind, payload_len) = match *self {
            Frame::DriverInput(_) => (FRAME_KIND_DRIVER_INPUT, DRIVER_INPUT_PAYLOAD_SIZE),
            Frame::Safety(_) => (FRAME_KIND_SAFETY, SAFETY_PAYLOAD_SIZE),
            Frame::Alive { .. } => (FRAME_KIND_ALIVE, ALIVE_PAYLOAD_SIZE),
            Frame::VehicleState(_) => (FRAME_KIND_VEHICLE_STATE, VEHICLE_STATE_PAYLOAD_SIZE),
            Frame::CmdDrive(_) => (FRAME_KIND_CMD_DRIVE, COMMAND_PAYLOAD_SIZE),
            Frame::CmdBrake(_) => (FRAME_KIND_CMD_BRAKE, COMMAND_PAYLOAD_SIZE),
            Frame::CmdSteer(_) => (FRAME_KIND_CMD_STEER, COMMAND_PAYLOAD_SIZE),
        };

        output.fill(0);
        output[..FRAME_WIRE_MAGIC.len()].copy_from_slice(&FRAME_WIRE_MAGIC);
        output[4] = FRAME_WIRE_VERSION;
        output[5] = kind;
        output[6] = payload_len as u8;
        let payload_start = FRAME_WIRE_HEADER_SIZE;
        let payload_end = payload_start + payload_len;
        let payload = &mut output[payload_start..payload_end];

        match *self {
            Frame::DriverInput(input) => {
                put_f32(payload, 0, input.throttle);
                put_f32(payload, 4, input.brake);
                put_f32(payload, 8, input.steer);
            }
            Frame::Safety(safety) => {
                payload[0] = u8::from(safety.estop);
                payload[1] = safety.system_state as u8;
            }
            Frame::Alive { index, counter } => {
                payload[0] = index as u8;
                payload[1..9].copy_from_slice(&counter.to_le_bytes());
            }
            Frame::VehicleState(state) => {
                for (index, value) in [
                    state.t,
                    state.x,
                    state.y,
                    state.yaw,
                    state.v,
                    state.yaw_rate,
                    state.wheel_omega,
                ]
                .into_iter()
                .enumerate()
                {
                    put_f32(payload, index * 4, value);
                }
            }
            Frame::CmdDrive(value) | Frame::CmdBrake(value) | Frame::CmdSteer(value) => {
                put_f32(payload, 0, value);
            }
        }

        let checksum_offset = payload_end;
        let checksum = frame_crc32(&output[..checksum_offset]);
        output[checksum_offset..checksum_offset + FRAME_WIRE_CRC_SIZE]
            .copy_from_slice(&checksum.to_le_bytes());
        Ok(checksum_offset + FRAME_WIRE_CRC_SIZE)
    }

    /// Decode exactly one complete wire frame.
    pub fn decode_wire(input: &[u8]) -> Result<Self, FrameWireError> {
        if input.len() < FRAME_WIRE_HEADER_SIZE + FRAME_WIRE_CRC_SIZE {
            return Err(FrameWireError::InvalidLength);
        }
        if input[..FRAME_WIRE_MAGIC.len()] != FRAME_WIRE_MAGIC[..] {
            return Err(FrameWireError::InvalidMagic);
        }
        if input[4] != FRAME_WIRE_VERSION {
            return Err(FrameWireError::UnsupportedVersion);
        }

        let kind = input[5];
        let expected_payload_len = match kind {
            FRAME_KIND_DRIVER_INPUT => DRIVER_INPUT_PAYLOAD_SIZE,
            FRAME_KIND_SAFETY => SAFETY_PAYLOAD_SIZE,
            FRAME_KIND_ALIVE => ALIVE_PAYLOAD_SIZE,
            FRAME_KIND_VEHICLE_STATE => VEHICLE_STATE_PAYLOAD_SIZE,
            FRAME_KIND_CMD_DRIVE | FRAME_KIND_CMD_BRAKE | FRAME_KIND_CMD_STEER => {
                COMMAND_PAYLOAD_SIZE
            }
            _ => return Err(FrameWireError::UnknownKind),
        };
        let payload_len = usize::from(input[6]);
        let checksum_offset = FRAME_WIRE_HEADER_SIZE + payload_len;
        if payload_len != expected_payload_len
            || input.len() != checksum_offset + FRAME_WIRE_CRC_SIZE
        {
            return Err(FrameWireError::InvalidLength);
        }

        let expected_checksum = u32::from_le_bytes(
            input[checksum_offset..checksum_offset + FRAME_WIRE_CRC_SIZE]
                .try_into()
                .map_err(|_| FrameWireError::InvalidLength)?,
        );
        if frame_crc32(&input[..checksum_offset]) != expected_checksum {
            return Err(FrameWireError::ChecksumMismatch);
        }

        let payload = &input[FRAME_WIRE_HEADER_SIZE..checksum_offset];
        match kind {
            FRAME_KIND_DRIVER_INPUT => Ok(Frame::DriverInput(DriverInput {
                throttle: read_f32(payload, 0)?,
                brake: read_f32(payload, 4)?,
                steer: read_f32(payload, 8)?,
            })),
            FRAME_KIND_SAFETY => {
                let estop = match payload[0] {
                    0 => false,
                    1 => true,
                    _ => return Err(FrameWireError::InvalidEnum),
                };
                let system_state = match payload[1] {
                    0 => SystemState::Normal,
                    1 => SystemState::Degraded,
                    2 => SystemState::EStop,
                    _ => return Err(FrameWireError::InvalidEnum),
                };
                Ok(Frame::Safety(Safety {
                    estop,
                    system_state,
                }))
            }
            FRAME_KIND_ALIVE => {
                let index = usize::from(payload[0]);
                if index >= ALIVE_NODES.len() {
                    return Err(FrameWireError::InvalidAliveIndex);
                }
                let counter = u64::from_le_bytes(
                    payload[1..9]
                        .try_into()
                        .map_err(|_| FrameWireError::InvalidLength)?,
                );
                Ok(Frame::Alive { index, counter })
            }
            FRAME_KIND_VEHICLE_STATE => Ok(Frame::VehicleState(VehicleState {
                t: read_f32(payload, 0)?,
                x: read_f32(payload, 4)?,
                y: read_f32(payload, 8)?,
                yaw: read_f32(payload, 12)?,
                v: read_f32(payload, 16)?,
                yaw_rate: read_f32(payload, 20)?,
                wheel_omega: read_f32(payload, 24)?,
            })),
            FRAME_KIND_CMD_DRIVE => Ok(Frame::CmdDrive(read_f32(payload, 0)?)),
            FRAME_KIND_CMD_BRAKE => Ok(Frame::CmdBrake(read_f32(payload, 0)?)),
            FRAME_KIND_CMD_STEER => Ok(Frame::CmdSteer(read_f32(payload, 0)?)),
            _ => Err(FrameWireError::UnknownKind),
        }
    }
}

fn put_f32(output: &mut [u8], offset: usize, value: f32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_f32(input: &[u8], offset: usize) -> Result<f32, FrameWireError> {
    let value = f32::from_le_bytes(
        input[offset..offset + 4]
            .try_into()
            .map_err(|_| FrameWireError::InvalidLength)?,
    );
    if !value.is_finite() {
        return Err(FrameWireError::NonFiniteValue);
    }
    Ok(value)
}

fn frame_crc32(bytes: &[u8]) -> u32 {
    const CRC32_REFLECTED_POLYNOMIAL: u32 = 0xedb8_8320;
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..u8::BITS {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ CRC32_REFLECTED_POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod frame_wire_tests {
    use super::*;

    fn encode(frame: Frame) -> ([u8; FRAME_WIRE_MAX_SIZE], usize) {
        let mut output = [0; FRAME_WIRE_MAX_SIZE];
        let length = frame.encode_wire(&mut output).unwrap();
        (output, length)
    }

    fn update_checksum(bytes: &mut [u8]) {
        let checksum_offset = bytes.len() - FRAME_WIRE_CRC_SIZE;
        let checksum = frame_crc32(&bytes[..checksum_offset]);
        bytes[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
    }

    #[test]
    fn every_frame_variant_round_trips_through_a_fixed_buffer() {
        let frames = [
            Frame::DriverInput(DriverInput {
                throttle: 0.5,
                brake: 0.25,
                steer: -0.75,
            }),
            Frame::Safety(Safety {
                estop: true,
                system_state: SystemState::EStop,
            }),
            Frame::Alive {
                index: ALIVE_STEERING_INDEX,
                counter: 0x1234_5678_9abc_def0,
            },
            Frame::VehicleState(VehicleState {
                t: 1.25,
                x: -2.5,
                y: 3.75,
                yaw: -0.5,
                v: 0.8,
                yaw_rate: -1.2,
                wheel_omega: 26.5,
            }),
            Frame::CmdDrive(1.25),
            Frame::CmdBrake(4.0),
            Frame::CmdSteer(-0.125),
        ];

        for frame in frames {
            let (output, length) = encode(frame);
            assert!(length <= FRAME_WIRE_MAX_SIZE);
            assert!(output[length..].iter().all(|byte| *byte == 0));
            assert_eq!(Frame::decode_wire(&output[..length]), Ok(frame));
        }
    }

    #[test]
    fn safety_state_and_boolean_fields_are_validated() {
        for state in [
            SystemState::Normal,
            SystemState::Degraded,
            SystemState::EStop,
        ] {
            let frame = Frame::Safety(Safety {
                estop: false,
                system_state: state,
            });
            let (bytes, length) = encode(frame);
            assert_eq!(Frame::decode_wire(&bytes[..length]), Ok(frame));
        }

        let (mut bytes, length) = encode(Frame::Safety(Safety::default()));
        bytes[FRAME_WIRE_HEADER_SIZE] = 2;
        update_checksum(&mut bytes[..length]);
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::InvalidEnum)
        );

        bytes[FRAME_WIRE_HEADER_SIZE] = 0;
        bytes[FRAME_WIRE_HEADER_SIZE + 1] = 3;
        update_checksum(&mut bytes[..length]);
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::InvalidEnum)
        );
    }

    #[test]
    fn unknown_version_kind_bad_length_and_magic_are_rejected() {
        let (mut bytes, length) = encode(Frame::CmdDrive(0.5));
        bytes[4] = FRAME_WIRE_VERSION.wrapping_add(1);
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::UnsupportedVersion)
        );

        bytes[4] = FRAME_WIRE_VERSION;
        bytes[5] = 0xff;
        update_checksum(&mut bytes[..length]);
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::UnknownKind)
        );

        let (mut bytes, length) = encode(Frame::CmdDrive(0.5));
        bytes[0] ^= 1;
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::InvalidMagic)
        );
        bytes[0] ^= 1;
        assert_eq!(
            Frame::decode_wire(&bytes[..length - 1]),
            Err(FrameWireError::InvalidLength)
        );
        assert_eq!(
            Frame::decode_wire(&bytes[..length + 1]),
            Err(FrameWireError::InvalidLength)
        );

        bytes[6] += 1;
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::InvalidLength)
        );
    }

    #[test]
    fn checksum_and_alive_indices_are_validated() {
        let (mut bytes, length) = encode(Frame::CmdBrake(0.5));
        bytes[FRAME_WIRE_HEADER_SIZE] ^= 1;
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::ChecksumMismatch)
        );

        let (mut bytes, length) = encode(Frame::Alive {
            index: ALIVE_ENGINE_INDEX,
            counter: 9,
        });
        bytes[FRAME_WIRE_HEADER_SIZE] = ALIVE_NODES.len() as u8;
        update_checksum(&mut bytes[..length]);
        assert_eq!(
            Frame::decode_wire(&bytes[..length]),
            Err(FrameWireError::InvalidAliveIndex)
        );
        assert_eq!(
            Frame::Alive {
                index: ALIVE_NODES.len(),
                counter: 0,
            }
            .encode_wire(&mut [0; FRAME_WIRE_MAX_SIZE]),
            Err(FrameWireError::InvalidAliveIndex)
        );
    }

    #[test]
    fn non_finite_values_are_rejected_when_encoding_and_decoding() {
        assert_eq!(
            Frame::CmdDrive(f32::NAN).encode_wire(&mut [0; FRAME_WIRE_MAX_SIZE]),
            Err(FrameWireError::NonFiniteValue)
        );
        assert_eq!(
            Frame::CmdBrake(f32::INFINITY).encode_wire(&mut [0; FRAME_WIRE_MAX_SIZE]),
            Err(FrameWireError::NonFiniteValue)
        );

        for value in [f32::NAN, f32::INFINITY] {
            let (mut bytes, length) = encode(Frame::CmdSteer(0.0));
            bytes[FRAME_WIRE_HEADER_SIZE..FRAME_WIRE_HEADER_SIZE + 4]
                .copy_from_slice(&value.to_le_bytes());
            update_checksum(&mut bytes[..length]);
            assert_eq!(
                Frame::decode_wire(&bytes[..length]),
                Err(FrameWireError::NonFiniteValue)
            );
        }

        let mut output = [0; FRAME_WIRE_MAX_SIZE];
        assert_eq!(
            Frame::DriverInput(DriverInput {
                throttle: 0.0,
                brake: 0.0,
                steer: f32::NEG_INFINITY,
            })
            .encode_wire(&mut output),
            Err(FrameWireError::NonFiniteValue)
        );
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
        Self {
            delay_ticks,
            loss_permille: 0,
            down: false,
            rng: 0x1234_5678,
            inflight: Vec::new(),
        }
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
            self.inflight.push(InFlight {
                due_tick: now_tick + delay,
                to,
                frame,
            });
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
