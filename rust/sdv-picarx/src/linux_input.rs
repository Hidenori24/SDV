//! Fresh vehicle snapshots received over a local Unix datagram socket.

use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::time::{Duration, Instant};

use sdv_core::model::VehicleState;
use sdv_core::platform::PlatformInputs;
use sdv_core::rte::DriverInput;
use serde::Deserialize;

use crate::InputSource;

const MAX_DATAGRAM_BYTES: usize = 2048;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidFreshnessLimit,
    InvalidPacket(serde_json::Error),
    NoInput,
    StaleInput,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFrame {
    sequence: u64,
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
    max_age: Duration,
    last_sequence: Option<u64>,
    last_frame: Option<(PlatformInputs, Instant)>,
}

impl UnixDatagramInputSource {
    pub fn bind(path: impl AsRef<Path>, max_age: Duration) -> Result<Self, Error> {
        if max_age.is_zero() {
            return Err(Error::InvalidFreshnessLimit);
        }
        let socket = UnixDatagram::bind(path).map_err(Error::Io)?;
        socket.set_nonblocking(true).map_err(Error::Io)?;
        Ok(Self {
            socket,
            max_age,
            last_sequence: None,
            last_frame: None,
        })
    }

    fn receive_latest(&mut self) -> Result<(), Error> {
        let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
        loop {
            match self.socket.recv(&mut buffer) {
                Ok(length) => {
                    let frame: InputFrame =
                        serde_json::from_slice(&buffer[..length]).map_err(Error::InvalidPacket)?;
                    if self
                        .last_sequence
                        .is_some_and(|last| frame.sequence <= last)
                    {
                        continue;
                    }
                    self.last_sequence = Some(frame.sequence);
                    self.last_frame = Some((frame.into(), Instant::now()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }
}

impl InputSource for UnixDatagramInputSource {
    type Error = Error;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        self.receive_latest()?;
        match self.last_frame {
            Some((frame, received_at)) if received_at.elapsed() <= self.max_age => Ok(frame),
            Some(_) => Err(Error::StaleInput),
            None => Err(Error::NoInput),
        }
    }
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

    const FRAME: &str = r#"{"sequence":1,"throttle":0.4,"brake":0.0,"steer":-0.1,"estop":false,"vehicle":{"t":1.0,"x":2.0,"y":3.0,"yaw":0.2,"v":0.5,"yaw_rate":0.1,"wheel_omega":5.0}}"#;

    #[test]
    fn receives_driver_and_vehicle_snapshot_from_local_peer() {
        let input_path = socket_path("input");
        let peer_path = socket_path("peer");
        let mut input = UnixDatagramInputSource::bind(&input_path, Duration::from_secs(1)).unwrap();
        let peer = UnixDatagram::bind(&peer_path).unwrap();
        peer.send_to(FRAME.as_bytes(), &input_path).unwrap();

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
        peer.send_to(FRAME.as_bytes(), &input_path).unwrap();
        let newer = FRAME
            .replace("\"sequence\":1", "\"sequence\":2")
            .replace("0.4", "0.8");
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
        let frame = format!("{}{}", &FRAME[..FRAME.len() - 1], ",\"ignored\":true}");
        assert!(matches!(serde_json::from_str::<InputFrame>(&frame), Err(_)));
        assert!(matches!(
            UnixDatagramInputSource::bind(socket_path("unused"), Duration::ZERO),
            Err(Error::InvalidFreshnessLimit)
        ));
    }
}
