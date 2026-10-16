//! Safety supervisor (watchdog), mirroring C++ `Swc::Safety` (`src/swc/safety_swc.cpp`).

use crate::model::*;

pub struct SafetySupervisor {
    params: SafetyParams,
    monitors: [HeartbeatMonitor; 4],
    /// A watchdog-triggered EStop is latched until `reset()` (like a real E-Stop).
    latched_estop: bool,
}

impl SafetySupervisor {
    pub fn new() -> Self {
        Self { params: SafetyParams::default(), monitors: [HeartbeatMonitor::default(); 4], latched_estop: false }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One 100 ms cycle. `alive` = alive counters of [engine, brake, steering, vehicledynamics];
    /// `estop_request` is the external request (UI / hardware).
    /// Returns the new `(estop, system_state)`.
    pub fn main_100ms(&mut self, alive: [u64; 4], dt_ms: u32, estop_request: bool) -> (bool, SafetyState) {
        let p = self.params;
        let mut ok = [true; 4];
        for (i, m) in self.monitors.iter_mut().enumerate() {
            ok[i] = update_heartbeat(m, alive[i], dt_ms, &p);
        }
        let wd_state = compute_safety_state(&HeartbeatStatus {
            engine_ok: ok[0],
            brake_ok: ok[1],
            steering_ok: ok[2],
            vehicledynamics_ok: ok[3],
        });
        if wd_state == SafetyState::EStop {
            self.latched_estop = true;
        }
        let estop = estop_request || self.latched_estop;
        (estop, if estop { SafetyState::EStop } else { wd_state })
    }
}

impl Default for SafetySupervisor {
    fn default() -> Self {
        Self::new()
    }
}
