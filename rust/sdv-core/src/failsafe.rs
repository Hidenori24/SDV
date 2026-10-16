//! Receiving-side fail-safe of an ECU (ADR-0002).
//!
//! An ECU never trusts a stale bus value for long:
//! - DriverInput silent for > 200 ms -> the driver input is treated as all zero
//! - Safety silent for > 100 ms      -> the ECU enters E-Stop behaviour by itself
//!   (so the car also stops when the VCU or the whole bus is gone)

use crate::rte::{DriverInput, Rte};

#[derive(Clone, Copy, Debug)]
pub struct FailsafeParams {
    /// Ticks (10 ms) of silence after which DriverInput is zeroed.
    pub driver_timeout_ticks: u32,
    /// Ticks (10 ms) of silence after which the ECU enters E-Stop on its own.
    pub safety_timeout_ticks: u32,
}

impl Default for FailsafeParams {
    fn default() -> Self {
        Self { driver_timeout_ticks: 20, safety_timeout_ticks: 10 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FailsafeStatus {
    pub driver_timeout: bool,
    pub safety_timeout: bool,
}

pub fn status(rte: &Rte, p: &FailsafeParams) -> FailsafeStatus {
    FailsafeStatus {
        driver_timeout: rte.rx_age.driver > p.driver_timeout_ticks,
        safety_timeout: rte.rx_age.safety > p.safety_timeout_ticks,
    }
}

/// Replace stale inputs by their safe values. A new frame overwrites them again, so the ECU
/// recovers by itself when the link comes back.
pub fn apply(rte: &mut Rte, p: &FailsafeParams) {
    let st = status(rte, p);
    if st.driver_timeout {
        rte.input = DriverInput::default();
    }
    if st.safety_timeout {
        rte.safety.estop = true;
    }
}
