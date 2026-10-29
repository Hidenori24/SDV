//! Fresh vehicle snapshots received over a local Unix datagram socket.

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sdv_core::model::VehicleState;
use sdv_core::platform::PlatformInputs;
use sdv_core::rte::DriverInput;
use serde::Deserialize;

use crate::InputSource;

const MAX_DATAGRAM_BYTES: usize = 2048;
const MAX_FRAMES_PER_SAMPLE: usize = 8;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidFreshnessLimit,
    InvalidPacket(serde_json::Error),
    OversizedPacket,
    TooManyQueuedFrames,
    FutureInputTimestamp,
    MonotonicClock(io::Error),
    NoInput,
    StaleInput,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFrame {
    sequence: u64,
    sampled_at_monotonic_ns: u64,
    throttle: f32,
    brake: f32,
    steer: f32,
    estop: bool,
    vehicle: StateFrame,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFrame {
    t: f32,
    x: f32,
    y: f32,
    yaw: f32,
    v: f32,
    yaw_rate: f32,
    wheel_omega: f32,
}

impl From<InputFrame> for PlatformInputs {
    fn from(frame: InputFrame) -> Self {
        Self {
            driver: DriverInput {
                throttle: frame.throttle,
                brake: frame.brake,
                steer: frame.steer,
            },
            vehicle: VehicleState {
                t: frame.vehicle.t,
                x: frame.vehicle.x,
                y: frame.vehicle.y,
                yaw: frame.vehicle.yaw,
                v: frame.vehicle.v,
                yaw_rate: frame.vehicle.yaw_rate,
                wheel_omega: frame.vehicle.wheel_omega,
            },
            estop: frame.estop,
        }
    }
}

/// Receives sequenced driver commands and measured or estimated vehicle state from a local peer.
///
/// A sample remains valid only up to the configured freshness limit. When the peer stops sending,
/// `sample` returns an error so the platform executor requests a motor stop. The state values must
/// come from an estimator or sensor source; this adapter does not fabricate odometry.
pub struct UnixDatagramInputSource {
    socket: UnixDatagram,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
    max_age: Duration,
    last_sequence: Option<u64>,
    last_frame: Option<(PlatformInputs, u64)>,
}

impl UnixDatagramInputSource {
    pub fn bind(path: impl AsRef<Path>, max_age: Duration) -> Result<Self, Error> {
        if max_age.is_zero() {
            return Err(Error::InvalidFreshnessLimit);
        }
        let socket_path = path.as_ref().to_path_buf();
        let socket = UnixDatagram::bind(&socket_path).map_err(Error::Io)?;
        let metadata = match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => metadata,
            Ok(_) => {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "bound input path is not a socket",
                )))
            }
            Err(error) => return Err(Error::Io(error)),
        };
        if let Err(error) = socket.set_nonblocking(true) {
            let _ = fs::remove_file(&socket_path);
            return Err(Error::Io(error));
        }
        Ok(Self {
            socket,
            socket_path,
            socket_identity: (metadata.dev(), metadata.ino()),
            max_age,
            last_sequence: None,
            last_frame: None,
        })
    }

    fn receive_latest(&mut self) -> Result<(), Error> {
        let mut buffer = [0_u8; MAX_DATAGRAM_BYTES + 1];
        let mut received_frames = 0;
        loop {
            match self.socket.recv(&mut buffer) {
                Ok(length) => {
                    if length > MAX_DATAGRAM_BYTES {
                        return Err(Error::OversizedPacket);
                    }
                    received_frames += 1;
                    if received_frames > MAX_FRAMES_PER_SAMPLE {
                        return Err(Error::TooManyQueuedFrames);
                    }
                    let frame: InputFrame =
                        serde_json::from_slice(&buffer[..length]).map_err(Error::InvalidPacket)?;
                    if self
                        .last_sequence
                        .is_some_and(|last| frame.sequence <= last)
                    {
                        continue;
                    }
                    self.last_sequence = Some(frame.sequence);
                    self.last_frame = Some((frame.into(), frame.sampled_at_monotonic_ns));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }
}

impl Drop for UnixDatagramInputSource {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path) {
            if metadata.file_type().is_socket()
                && (metadata.dev(), metadata.ino()) == self.socket_identity
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }
}

impl InputSource for UnixDatagramInputSource {
    type Error = Error;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        self.receive_latest()?;
        let now = monotonic_time_ns()?;
        match self.last_frame {
            Some((frame, sampled_at)) if sampled_at <= now => {
                if u128::from(now - sampled_at) <= self.max_age.as_nanos() {
                    Ok(frame)
                } else {
                    Err(Error::StaleInput)
                }
            }
            Some(_) => Err(Error::FutureInputTimestamp),
            None => Err(Error::NoInput),
        }
    }
}

fn monotonic_time_ns() -> Result<u64, Error> {
    let mut now = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `clock_gettime` initializes the provided timespec when it returns success.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, now.as_mut_ptr()) } != 0 {
        return Err(Error::MonotonicClock(io::Error::last_os_error()));
    }
    // SAFETY: the successful call above initialized `now`.
    let now = unsafe { now.assume_init() };
    let seconds = u64::try_from(now.tv_sec).map_err(|_| {
        Error::MonotonicClock(io::Error::new(
            io::ErrorKind::InvalidData,
            "monotonic clock returned negative seconds",
        ))
    })?;
    let nanoseconds = u64::try_from(now.tv_nsec).map_err(|_| {
        Error::MonotonicClock(io::Error::new(
            io::ErrorKind::InvalidData,
            "monotonic clock returned negative nanoseconds",
        ))
    })?;
    seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|value| value.checked_add(nanoseconds))
        .ok_or_else(|| {
            Error::MonotonicClock(io::Error::new(
                io::ErrorKind::InvalidData,
                "monotonic clock value is out of range",
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn socket_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "sdv-picarx-{label}-{}-{}.sock",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn frame(sequence: u64, throttle: f32, sampled_at_monotonic_ns: u64) -> String {
        format!(
            r#"{{"sequence":{sequence},"sampled_at_monotonic_ns":{sampled_at_monotonic_ns},"throttle":{throttle},"brake":0.0,"steer":-0.1,"estop":false,"vehicle":{{"t":1.0,"x":2.0,"y":3.0,"yaw":0.2,"v":0.5,"yaw_rate":0.1,"wheel_omega":5.0}}}}"#
        )
    }

    #[test]
    fn receives_driver_and_vehicle_snapshot_from_local_peer() {
        let input_path = socket_path("input");
        let peer_path = socket_path("peer");
        let mut input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        let peer = UnixDatagram::bind(&peer_path).unwrap();
        let frame = frame(1, 0.4, monotonic_time_ns().unwrap());
        peer.send_to(frame.as_bytes(), &input_path).unwrap();

        let sample = input.sample().unwrap();
        assert_eq!(sample.driver.throttle, 0.4);
        assert_eq!(sample.driver.steer, -0.1);
        assert_eq!(sample.vehicle.v, 0.5);
        assert!(!sample.estop);

        drop(input);
        drop(peer);
        let _ = std::fs::remove_file(input_path);
        let _ = std::fs::remove_file(peer_path);
    }

    #[test]
    fn newest_sequence_wins_and_stale_or_missing_input_is_rejected() {
        let input_path = socket_path("input");
        let peer_path = socket_path("peer");
        let mut input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        let peer = UnixDatagram::bind(&peer_path).unwrap();
        let older = frame(1, 0.4, monotonic_time_ns().unwrap());
        let newer = frame(2, 0.8, monotonic_time_ns().unwrap());
        peer.send_to(older.as_bytes(), &input_path).unwrap();
        peer.send_to(newer.as_bytes(), &input_path).unwrap();

        assert_eq!(input.sample().unwrap().driver.throttle, 0.8);
        input.max_age = Duration::from_nanos(1);
        std::thread::sleep(Duration::from_millis(1));
        assert!(matches!(input.sample(), Err(Error::StaleInput)));

        drop(input);
        drop(peer);
        let _ = std::fs::remove_file(input_path);
        let _ = std::fs::remove_file(peer_path);
    }

    #[test]
    fn rejects_unknown_fields_and_zero_freshness_limit() {
        let packet = frame(1, 0.4, monotonic_time_ns().unwrap());
        let packet = format!("{}{}", &packet[..packet.len() - 1], ",\"ignored\":true}");
        assert!(matches!(
            serde_json::from_str::<InputFrame>(&packet),
            Err(_)
        ));
        assert!(matches!(
            UnixDatagramInputSource::bind(socket_path("unused"), Duration::ZERO),
            Err(Error::InvalidFreshnessLimit)
        ));
    }

    #[test]
    fn rejects_stale_and_future_source_timestamps() {
        let input_path = socket_path("input");
        let peer_path = socket_path("peer");
        let mut input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        let peer = UnixDatagram::bind(&peer_path).unwrap();
        let now = monotonic_time_ns().unwrap();
        let stale = frame(1, 0.4, now.saturating_sub(NANOS_PER_SECOND * 2));
        peer.send_to(stale.as_bytes(), &input_path).unwrap();
        assert!(matches!(input.sample(), Err(Error::StaleInput)));

        let future = frame(2, 0.4, monotonic_time_ns().unwrap() + NANOS_PER_SECOND);
        peer.send_to(future.as_bytes(), &input_path).unwrap();
        assert!(matches!(input.sample(), Err(Error::FutureInputTimestamp)));

        drop(input);
        drop(peer);
        let _ = std::fs::remove_file(input_path);
        let _ = std::fs::remove_file(peer_path);
    }

    #[test]
    fn rejects_oversized_and_excessively_queued_datagrams() {
        let input_path = socket_path("input");
        let peer_path = socket_path("peer");
        let mut input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        let peer = UnixDatagram::bind(&peer_path).unwrap();
        let current = frame(1, 0.4, monotonic_time_ns().unwrap());
        let mut oversized = current.into_bytes();
        oversized.resize(MAX_DATAGRAM_BYTES + 1, b' ');
        peer.send_to(&oversized, &input_path).unwrap();
        assert!(matches!(input.sample(), Err(Error::OversizedPacket)));

        for sequence in 1..=(MAX_FRAMES_PER_SAMPLE as u64 + 1) {
            let packet = frame(sequence, 0.4, monotonic_time_ns().unwrap());
            peer.send_to(packet.as_bytes(), &input_path).unwrap();
        }
        assert!(matches!(input.sample(), Err(Error::TooManyQueuedFrames)));

        drop(input);
        drop(peer);
        let _ = std::fs::remove_file(input_path);
        let _ = std::fs::remove_file(peer_path);
    }

    #[test]
    fn removes_its_socket_path_when_dropped() {
        let input_path = socket_path("input");
        let input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        assert!(input_path.exists());
        drop(input);
        assert!(!input_path.exists());
    }
}
