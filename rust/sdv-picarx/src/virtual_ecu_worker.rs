//! Linux Unix-datagram adapter for one isolated virtual ECU task.
//!
//! Each worker owns exactly one VirtualEcuTask and processes one request per 10 ms cycle.
//! This module has no GPIO or I2C access: a future vehicle coordinator remains the sole owner of
//! physical input and actuator I/O.

use std::fmt;
use std::io;
use std::os::unix::net::UnixDatagram;

use sdv_core::bus::{Frame, FrameWireError, Node, FRAME_WIRE_MAX_SIZE};
use sdv_core::config::Config;
use sdv_core::runtime::{EcuTaskOutput, VirtualEcuId, VirtualEcuTask};

pub const WORKER_PROTOCOL_VERSION: u8 = 1;
pub const MAX_INPUT_FRAMES: usize = 2;
const REQUEST_MAGIC: [u8; 4] = *b"SDVQ";
const RESPONSE_MAGIC: [u8; 4] = *b"SDVP";
const HEADER_SIZE: usize = 15;
const CHECKSUM_SIZE: usize = 4;
const CYCLE_ID_START: usize = 6;
const FRAME_COUNT_OFFSET: usize = 14;
const FRAME_RECORD_OVERHEAD: usize = 1;

/// Maximum request or response datagram size: fixed header, two maximal frames, and CRC32.
pub const WORKER_DATAGRAM_MAX_SIZE: usize =
    HEADER_SIZE + MAX_INPUT_FRAMES * (FRAME_RECORD_OVERHEAD + FRAME_WIRE_MAX_SIZE) + CHECKSUM_SIZE;

/// Failure to decode a worker packet or advance a worker safely.
#[derive(Debug)]
pub enum WorkerError {
    Io(io::Error),
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion,
    InvalidEcuId,
    WrongEcu,
    NonIncreasingCycle,
    TooManyInputFrames,
    WrongFrameReceiver,
    InvalidResponseFrames,
    ChecksumMismatch,
    FrameWire(FrameWireError),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "worker socket I/O failed: {error}"),
            Self::InvalidLength => formatter.write_str("invalid worker datagram length"),
            Self::InvalidMagic => formatter.write_str("invalid worker datagram magic"),
            Self::UnsupportedVersion => formatter.write_str("unsupported worker protocol version"),
            Self::InvalidEcuId => formatter.write_str("invalid worker ECU id"),
            Self::WrongEcu => formatter.write_str("request is addressed to a different ECU"),
            Self::NonIncreasingCycle => formatter.write_str("worker cycle id did not increase"),
            Self::TooManyInputFrames => {
                formatter.write_str("request contains more than two frames")
            }
            Self::WrongFrameReceiver => {
                formatter.write_str("input frame is not addressed to this ECU")
            }
            Self::InvalidResponseFrames => {
                formatter.write_str("worker response frames do not match its ECU")
            }
            Self::ChecksumMismatch => formatter.write_str("worker datagram checksum mismatch"),
            Self::FrameWire(error) => write!(formatter, "invalid bus frame: {error:?}"),
        }
    }
}

impl std::error::Error for WorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for WorkerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkerResponse {
    pub ecu_id: VirtualEcuId,
    pub cycle_id: u64,
    pub command: Frame,
    pub alive: Frame,
}

/// One ECU worker. Socket creation and filesystem permissions belong to its caller.
pub struct VirtualEcuWorker {
    ecu_id: VirtualEcuId,
    task: VirtualEcuTask,
    last_cycle_id: Option<u64>,
}

impl VirtualEcuWorker {
    pub fn new(ecu_id: VirtualEcuId, config: &Config) -> Self {
        Self {
            ecu_id,
            task: VirtualEcuTask::new(ecu_id, config),
            last_cycle_id: None,
        }
    }

    pub fn ecu_id(&self) -> VirtualEcuId {
        self.ecu_id
    }

    /// Handle one bounded request and encode its response in a caller-owned buffer.
    ///
    /// Validation completes before the ECU task advances. A malformed, misrouted, duplicate, or
    /// stale request therefore has no effect on task state or the cycle counter.
    pub fn handle_datagram(
        &mut self,
        request: &[u8],
        response: &mut [u8; WORKER_DATAGRAM_MAX_SIZE],
    ) -> Result<usize, WorkerError> {
        let decoded = decode_request(request)?;
        if decoded.ecu_id != self.ecu_id {
            return Err(WorkerError::WrongEcu);
        }
        if self
            .last_cycle_id
            .is_some_and(|previous| decoded.cycle_id <= previous)
        {
            return Err(WorkerError::NonIncreasingCycle);
        }

        let output = self
            .task
            .step_10ms(&decoded.frames[..decoded.frame_count])
            .ok_or(WorkerError::InvalidResponseFrames)?;
        self.last_cycle_id = Some(decoded.cycle_id);
        encode_response(self.ecu_id, decoded.cycle_id, output, response)
    }

    /// Receive and answer one connected Unix datagram request.
    ///
    /// Oversized or invalid datagrams are dropped by returning an error; no response is sent and
    /// the task is left unchanged. The caller can log the error and continue serving.
    pub fn serve_once(&mut self, socket: &UnixDatagram) -> Result<(), WorkerError> {
        let mut request = [0_u8; WORKER_DATAGRAM_MAX_SIZE + 1];
        let received = socket.recv(&mut request)?;
        if received > WORKER_DATAGRAM_MAX_SIZE {
            return Err(WorkerError::InvalidLength);
        }

        let mut response = [0_u8; WORKER_DATAGRAM_MAX_SIZE];
        let response_len = self.handle_datagram(&request[..received], &mut response)?;
        socket.send(&response[..response_len])?;
        Ok(())
    }
}

/// Encode a bounded request for a worker.
///
/// The worker accepts only frames whose bus receiver set includes the selected ECU. With the
/// current bus matrix this permits DriverInput and Safety frames.
pub fn encode_request(
    ecu_id: VirtualEcuId,
    cycle_id: u64,
    frames: &[Frame],
    output: &mut [u8; WORKER_DATAGRAM_MAX_SIZE],
) -> Result<usize, WorkerError> {
    if frames.len() > MAX_INPUT_FRAMES {
        return Err(WorkerError::TooManyInputFrames);
    }

    output.fill(0);
    output[..4].copy_from_slice(&REQUEST_MAGIC);
    output[4] = WORKER_PROTOCOL_VERSION;
    output[5] = encode_ecu_id(ecu_id);
    output[CYCLE_ID_START..CYCLE_ID_START + 8].copy_from_slice(&cycle_id.to_le_bytes());
    output[FRAME_COUNT_OFFSET] = frames.len() as u8;

    let node = ecu_node(ecu_id);
    let mut cursor = HEADER_SIZE;
    for frame in frames {
        if !frame.receivers().contains(&node) {
            return Err(WorkerError::WrongFrameReceiver);
        }
        let mut encoded = [0_u8; FRAME_WIRE_MAX_SIZE];
        let frame_len = frame
            .encode_wire(&mut encoded)
            .map_err(WorkerError::FrameWire)?;
        output[cursor] = frame_len as u8;
        cursor += FRAME_RECORD_OVERHEAD;
        output[cursor..cursor + frame_len].copy_from_slice(&encoded[..frame_len]);
        cursor += frame_len;
    }

    append_checksum(output, cursor)
}

/// Decode one worker response and verify its ECU-specific command and heartbeat.
pub fn decode_response(datagram: &[u8]) -> Result<WorkerResponse, WorkerError> {
    if datagram.len() < HEADER_SIZE + 2 * FRAME_RECORD_OVERHEAD + CHECKSUM_SIZE
        || datagram.len() > WORKER_DATAGRAM_MAX_SIZE
    {
        return Err(WorkerError::InvalidLength);
    }
    validate_envelope(datagram, RESPONSE_MAGIC)?;
    let ecu_id = decode_ecu_id(datagram[5])?;
    let cycle_id = read_cycle_id(datagram);
    let frame_count = usize::from(datagram[FRAME_COUNT_OFFSET]);
    if frame_count != 2 {
        return Err(WorkerError::InvalidResponseFrames);
    }

    let checksum_offset = datagram.len() - CHECKSUM_SIZE;
    let mut cursor = HEADER_SIZE;
    let command = decode_frame_record(datagram, &mut cursor, checksum_offset)?;
    let alive = decode_frame_record(datagram, &mut cursor, checksum_offset)?;
    if cursor != checksum_offset || !valid_response_frames(ecu_id, command, alive) {
        return Err(WorkerError::InvalidResponseFrames);
    }

    Ok(WorkerResponse {
        ecu_id,
        cycle_id,
        command,
        alive,
    })
}

#[derive(Clone, Copy)]
struct DecodedRequest {
    ecu_id: VirtualEcuId,
    cycle_id: u64,
    frames: [Frame; MAX_INPUT_FRAMES],
    frame_count: usize,
}

fn decode_request(datagram: &[u8]) -> Result<DecodedRequest, WorkerError> {
    if datagram.len() < HEADER_SIZE + CHECKSUM_SIZE || datagram.len() > WORKER_DATAGRAM_MAX_SIZE {
        return Err(WorkerError::InvalidLength);
    }
    validate_envelope(datagram, REQUEST_MAGIC)?;
    let ecu_id = decode_ecu_id(datagram[5])?;
    let cycle_id = read_cycle_id(datagram);
    let frame_count = usize::from(datagram[FRAME_COUNT_OFFSET]);
    if frame_count > MAX_INPUT_FRAMES {
        return Err(WorkerError::TooManyInputFrames);
    }

    let checksum_offset = datagram.len() - CHECKSUM_SIZE;
    let mut cursor = HEADER_SIZE;
    let mut frames = [Frame::Safety(Default::default()); MAX_INPUT_FRAMES];
    let node = ecu_node(ecu_id);
    for frame in frames.iter_mut().take(frame_count) {
        let decoded = decode_frame_record(datagram, &mut cursor, checksum_offset)?;
        if !decoded.receivers().contains(&node) {
            return Err(WorkerError::WrongFrameReceiver);
        }
        *frame = decoded;
    }
    if cursor != checksum_offset {
        return Err(WorkerError::InvalidLength);
    }

    Ok(DecodedRequest {
        ecu_id,
        cycle_id,
        frames,
        frame_count,
    })
}

fn validate_envelope(datagram: &[u8], expected_magic: [u8; 4]) -> Result<(), WorkerError> {
    if datagram[..4] != expected_magic {
        return Err(WorkerError::InvalidMagic);
    }
    if datagram[4] != WORKER_PROTOCOL_VERSION {
        return Err(WorkerError::UnsupportedVersion);
    }
    let checksum_offset = datagram.len() - CHECKSUM_SIZE;
    let expected = u32::from_le_bytes(
        datagram[checksum_offset..]
            .try_into()
            .map_err(|_| WorkerError::InvalidLength)?,
    );
    if worker_crc32(&datagram[..checksum_offset]) != expected {
        return Err(WorkerError::ChecksumMismatch);
    }
    Ok(())
}

fn decode_frame_record(
    datagram: &[u8],
    cursor: &mut usize,
    checksum_offset: usize,
) -> Result<Frame, WorkerError> {
    if *cursor >= checksum_offset {
        return Err(WorkerError::InvalidLength);
    }
    let frame_len = usize::from(datagram[*cursor]);
    *cursor += FRAME_RECORD_OVERHEAD;
    if frame_len == 0 || frame_len > FRAME_WIRE_MAX_SIZE {
        return Err(WorkerError::InvalidLength);
    }
    let frame_end = (*cursor)
        .checked_add(frame_len)
        .ok_or(WorkerError::InvalidLength)?;
    if frame_end > checksum_offset {
        return Err(WorkerError::InvalidLength);
    }
    let frame =
        Frame::decode_wire(&datagram[*cursor..frame_end]).map_err(WorkerError::FrameWire)?;
    *cursor = frame_end;
    Ok(frame)
}

fn encode_response(
    ecu_id: VirtualEcuId,
    cycle_id: u64,
    output: EcuTaskOutput,
    response: &mut [u8; WORKER_DATAGRAM_MAX_SIZE],
) -> Result<usize, WorkerError> {
    if !valid_response_frames(ecu_id, output.command, output.alive) {
        return Err(WorkerError::InvalidResponseFrames);
    }
    response.fill(0);
    response[..4].copy_from_slice(&RESPONSE_MAGIC);
    response[4] = WORKER_PROTOCOL_VERSION;
    response[5] = encode_ecu_id(ecu_id);
    response[CYCLE_ID_START..CYCLE_ID_START + 8].copy_from_slice(&cycle_id.to_le_bytes());
    response[FRAME_COUNT_OFFSET] = 2;

    let mut cursor = HEADER_SIZE;
    for frame in [output.command, output.alive] {
        let mut encoded = [0_u8; FRAME_WIRE_MAX_SIZE];
        let frame_len = frame
            .encode_wire(&mut encoded)
            .map_err(WorkerError::FrameWire)?;
        response[cursor] = frame_len as u8;
        cursor += FRAME_RECORD_OVERHEAD;
        response[cursor..cursor + frame_len].copy_from_slice(&encoded[..frame_len]);
        cursor += frame_len;
    }
    append_checksum(response, cursor)
}

fn append_checksum(
    output: &mut [u8; WORKER_DATAGRAM_MAX_SIZE],
    payload_end: usize,
) -> Result<usize, WorkerError> {
    let checksum_end = payload_end
        .checked_add(CHECKSUM_SIZE)
        .ok_or(WorkerError::InvalidLength)?;
    if checksum_end > output.len() {
        return Err(WorkerError::InvalidLength);
    }
    let checksum = worker_crc32(&output[..payload_end]);
    output[payload_end..checksum_end].copy_from_slice(&checksum.to_le_bytes());
    Ok(checksum_end)
}

fn valid_response_frames(ecu_id: VirtualEcuId, command: Frame, alive: Frame) -> bool {
    let command_matches = matches!(
        (ecu_id, command),
        (VirtualEcuId::Engine, Frame::CmdDrive(_))
            | (VirtualEcuId::Brake, Frame::CmdBrake(_))
            | (VirtualEcuId::Steering, Frame::CmdSteer(_))
    );
    let alive_matches = matches!(
        (ecu_id, alive),
        (VirtualEcuId::Engine, Frame::Alive { index: 0, .. })
            | (VirtualEcuId::Brake, Frame::Alive { index: 1, .. })
            | (VirtualEcuId::Steering, Frame::Alive { index: 2, .. })
    );
    command_matches && alive_matches
}

fn encode_ecu_id(ecu_id: VirtualEcuId) -> u8 {
    match ecu_id {
        VirtualEcuId::Engine => 1,
        VirtualEcuId::Brake => 2,
        VirtualEcuId::Steering => 3,
    }
}

fn decode_ecu_id(value: u8) -> Result<VirtualEcuId, WorkerError> {
    match value {
        1 => Ok(VirtualEcuId::Engine),
        2 => Ok(VirtualEcuId::Brake),
        3 => Ok(VirtualEcuId::Steering),
        _ => Err(WorkerError::InvalidEcuId),
    }
}

fn ecu_node(ecu_id: VirtualEcuId) -> Node {
    match ecu_id {
        VirtualEcuId::Engine => Node::Engine,
        VirtualEcuId::Brake => Node::Brake,
        VirtualEcuId::Steering => Node::Steering,
    }
}

fn read_cycle_id(datagram: &[u8]) -> u64 {
    u64::from_le_bytes(
        datagram[CYCLE_ID_START..CYCLE_ID_START + 8]
            .try_into()
            .unwrap(),
    )
}

fn worker_crc32(bytes: &[u8]) -> u32 {
    const POLYNOMIAL: u32 = 0xedb8_8320;
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..u8::BITS {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdv_core::rte::{DriverInput, Safety};
    use std::os::unix::net::UnixDatagram;
    use std::thread;
    use std::time::Duration;

    fn make_request(
        ecu_id: VirtualEcuId,
        cycle_id: u64,
        frames: &[Frame],
    ) -> ([u8; WORKER_DATAGRAM_MAX_SIZE], usize) {
        let mut output = [0; WORKER_DATAGRAM_MAX_SIZE];
        let length = encode_request(ecu_id, cycle_id, frames, &mut output).unwrap();
        (output, length)
    }

    fn update_checksum(datagram: &mut [u8]) {
        let checksum_offset = datagram.len() - CHECKSUM_SIZE;
        let checksum = worker_crc32(&datagram[..checksum_offset]);
        datagram[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
    }

    fn input_frames(throttle: f32) -> [Frame; 2] {
        [
            Frame::Safety(Safety::default()),
            Frame::DriverInput(DriverInput {
                throttle,
                brake: 0.0,
                steer: 0.1,
            }),
        ]
    }

    #[test]
    fn unix_datagram_pair_round_trips_multiple_increasing_cycles() {
        let (worker_socket, coordinator_socket) = UnixDatagram::pair().unwrap();
        coordinator_socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let worker = thread::spawn(move || {
            let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
            worker.serve_once(&worker_socket).unwrap();
            worker.serve_once(&worker_socket).unwrap();
        });

        for cycle_id in [4, 5] {
            let frames = input_frames(0.4);
            let (request, request_len) = make_request(VirtualEcuId::Engine, cycle_id, &frames);
            coordinator_socket.send(&request[..request_len]).unwrap();

            let mut response = [0; WORKER_DATAGRAM_MAX_SIZE + 1];
            let response_len = coordinator_socket.recv(&mut response).unwrap();
            let response = decode_response(&response[..response_len]).unwrap();
            assert_eq!(response.ecu_id, VirtualEcuId::Engine);
            assert_eq!(response.cycle_id, cycle_id);
            assert!(matches!(response.command, Frame::CmdDrive(_)));
            assert_eq!(
                response.alive,
                Frame::Alive {
                    index: 0,
                    counter: if cycle_id == 4 { 1 } else { 2 },
                }
            );
        }
        worker.join().unwrap();
    }

    #[test]
    fn independent_workers_keep_their_own_ecu_and_alive_identity() {
        for ecu_id in [
            VirtualEcuId::Engine,
            VirtualEcuId::Brake,
            VirtualEcuId::Steering,
        ] {
            let mut worker = VirtualEcuWorker::new(ecu_id, &Config::default());
            for cycle_id in [10, 12] {
                let frames = input_frames(0.25);
                let (request, request_len) = make_request(ecu_id, cycle_id, &frames);
                let mut response_bytes = [0; WORKER_DATAGRAM_MAX_SIZE];
                let response_len = worker
                    .handle_datagram(&request[..request_len], &mut response_bytes)
                    .unwrap();
                let response = decode_response(&response_bytes[..response_len]).unwrap();
                assert_eq!(response.ecu_id, ecu_id);
                assert_eq!(response.cycle_id, cycle_id);
                assert!(matches!(
                    (ecu_id, response.command),
                    (VirtualEcuId::Engine, Frame::CmdDrive(_))
                        | (VirtualEcuId::Brake, Frame::CmdBrake(_))
                        | (VirtualEcuId::Steering, Frame::CmdSteer(_))
                ));
                assert_eq!(
                    response.alive,
                    Frame::Alive {
                        index: encode_ecu_id(ecu_id) as usize - 1,
                        counter: if cycle_id == 10 { 1 } else { 2 },
                    }
                );
            }
        }
    }

    #[test]
    fn wrong_ecu_duplicate_and_reversed_cycles_do_not_advance_task() {
        let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
        let frames = input_frames(0.3);
        let (wrong_ecu, wrong_len) = make_request(VirtualEcuId::Brake, 8, &frames);
        let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];
        assert!(matches!(
            worker.handle_datagram(&wrong_ecu[..wrong_len], &mut response),
            Err(WorkerError::WrongEcu)
        ));

        let (first, first_len) = make_request(VirtualEcuId::Engine, 8, &frames);
        let first_response_len = worker
            .handle_datagram(&first[..first_len], &mut response)
            .unwrap();
        assert_eq!(
            decode_response(&response[..first_response_len])
                .unwrap()
                .alive,
            Frame::Alive {
                index: 0,
                counter: 1,
            }
        );
        for cycle_id in [8, 7] {
            let (request, request_len) = make_request(VirtualEcuId::Engine, cycle_id, &frames);
            assert!(matches!(
                worker.handle_datagram(&request[..request_len], &mut response),
                Err(WorkerError::NonIncreasingCycle)
            ));
        }

        let (next, next_len) = make_request(VirtualEcuId::Engine, 9, &frames);
        let next_response_len = worker
            .handle_datagram(&next[..next_len], &mut response)
            .unwrap();
        assert_eq!(
            decode_response(&response[..next_response_len])
                .unwrap()
                .alive,
            Frame::Alive {
                index: 0,
                counter: 2,
            }
        );
    }

    #[test]
    fn malformed_lengths_checksums_and_misrouted_frames_are_rejected_without_advancing() {
        let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
        let frames = input_frames(0.5);
        let (valid, valid_len) = make_request(VirtualEcuId::Engine, 20, &frames);
        let mut response = [0; WORKER_DATAGRAM_MAX_SIZE];

        assert!(matches!(
            worker.handle_datagram(&valid[..valid_len - 1], &mut response),
            Err(WorkerError::InvalidLength | WorkerError::ChecksumMismatch)
        ));

        let mut bad_length = valid;
        bad_length[HEADER_SIZE] = u8::MAX;
        update_checksum(&mut bad_length[..valid_len]);
        assert!(matches!(
            worker.handle_datagram(&bad_length[..valid_len], &mut response),
            Err(WorkerError::InvalidLength)
        ));

        let mut bad_frame_checksum = valid;
        bad_frame_checksum[HEADER_SIZE + FRAME_RECORD_OVERHEAD + 7] ^= 1;
        update_checksum(&mut bad_frame_checksum[..valid_len]);
        assert!(matches!(
            worker.handle_datagram(&bad_frame_checksum[..valid_len], &mut response),
            Err(WorkerError::FrameWire(FrameWireError::ChecksumMismatch))
        ));

        let mut misrouted = [0; WORKER_DATAGRAM_MAX_SIZE];
        let mut encoded_frame = [0; FRAME_WIRE_MAX_SIZE];
        let frame_len = Frame::CmdDrive(1.0)
            .encode_wire(&mut encoded_frame)
            .unwrap();
        misrouted[..4].copy_from_slice(&REQUEST_MAGIC);
        misrouted[4] = WORKER_PROTOCOL_VERSION;
        misrouted[5] = encode_ecu_id(VirtualEcuId::Engine);
        misrouted[CYCLE_ID_START..CYCLE_ID_START + 8].copy_from_slice(&21_u64.to_le_bytes());
        misrouted[FRAME_COUNT_OFFSET] = 1;
        misrouted[HEADER_SIZE] = frame_len as u8;
        misrouted
            [HEADER_SIZE + FRAME_RECORD_OVERHEAD..HEADER_SIZE + FRAME_RECORD_OVERHEAD + frame_len]
            .copy_from_slice(&encoded_frame[..frame_len]);
        let misrouted_len = append_checksum(
            &mut misrouted,
            HEADER_SIZE + FRAME_RECORD_OVERHEAD + frame_len,
        )
        .unwrap();
        assert!(matches!(
            worker.handle_datagram(&misrouted[..misrouted_len], &mut response),
            Err(WorkerError::WrongFrameReceiver)
        ));

        let (next, next_len) = make_request(VirtualEcuId::Engine, 20, &frames);
        let next_response_len = worker
            .handle_datagram(&next[..next_len], &mut response)
            .unwrap();
        assert_eq!(
            decode_response(&response[..next_response_len])
                .unwrap()
                .alive,
            Frame::Alive {
                index: 0,
                counter: 1,
            }
        );
    }

    #[test]
    fn request_encoder_rejects_more_than_two_frames_and_wrong_receivers() {
        let mut output = [0; WORKER_DATAGRAM_MAX_SIZE];
        let too_many = [Frame::Safety(Safety::default()); MAX_INPUT_FRAMES + 1];
        assert!(matches!(
            encode_request(VirtualEcuId::Engine, 1, &too_many, &mut output),
            Err(WorkerError::TooManyInputFrames)
        ));
        assert!(matches!(
            encode_request(
                VirtualEcuId::Engine,
                1,
                &[Frame::CmdDrive(0.0)],
                &mut output
            ),
            Err(WorkerError::WrongFrameReceiver)
        ));
    }

    #[test]
    fn oversized_datagrams_are_rejected_by_socket_adapter() {
        let (worker_socket, coordinator_socket) = UnixDatagram::pair().unwrap();
        let mut worker = VirtualEcuWorker::new(VirtualEcuId::Engine, &Config::default());
        let sender = thread::spawn(move || {
            coordinator_socket
                .send(&vec![0_u8; WORKER_DATAGRAM_MAX_SIZE + 2])
                .unwrap();
        });
        let result = worker.serve_once(&worker_socket);
        sender.join().unwrap();
        assert!(matches!(result, Err(WorkerError::InvalidLength)));
    }
}
