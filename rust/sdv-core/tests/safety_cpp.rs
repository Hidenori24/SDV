//! Rust port of `tests/test_safety_logic.cpp` and the watchdog part of
//! `tests/test_safety_swc.cpp` (C++ is the reference implementation).

use sdv_core::model::{
    compute_safety_state, count_failed, update_heartbeat, HeartbeatMonitor, HeartbeatStatus,
    SafetyParams, SafetyState,
};
use sdv_core::safety::SafetySupervisor;

fn all_ok() -> HeartbeatStatus {
    HeartbeatStatus::default()
}

#[test]
fn all_alive_is_normal() {
    assert_eq!(compute_safety_state(&all_ok()), SafetyState::Normal);
}

#[test]
fn one_dead_swc_is_degraded_for_each_swc() {
    let cases = [
        HeartbeatStatus { engine_ok: false, ..all_ok() },
        HeartbeatStatus { brake_ok: false, ..all_ok() },
        HeartbeatStatus { steering_ok: false, ..all_ok() },
        HeartbeatStatus { vehicledynamics_ok: false, ..all_ok() },
    ];
    for hb in cases {
        assert_eq!(count_failed(&hb), 1);
        assert_eq!(compute_safety_state(&hb), SafetyState::Degraded);
    }
}

#[test]
fn two_or_more_dead_swcs_is_estop() {
    let mut hb = HeartbeatStatus { engine_ok: false, brake_ok: false, ..all_ok() };
    assert_eq!(compute_safety_state(&hb), SafetyState::EStop);
    hb.steering_ok = false;
    hb.vehicledynamics_ok = false;
    assert_eq!(count_failed(&hb), 4);
    assert_eq!(compute_safety_state(&hb), SafetyState::EStop);
}

#[test]
fn advancing_counter_keeps_swc_alive() {
    let p = SafetyParams::default();
    let mut m = HeartbeatMonitor::default();
    for c in 1..=100u64 {
        assert!(update_heartbeat(&mut m, c, 100, &p));
        assert_eq!(m.silent_ms, 0);
    }
}

#[test]
fn silence_beyond_timeout_is_detected() {
    let p = SafetyParams::default(); // 1000 ms
    let mut m = HeartbeatMonitor::default();
    assert!(update_heartbeat(&mut m, 1, 100, &p));
    for _ in 0..10 {
        assert!(update_heartbeat(&mut m, 1, 100, &p));
    }
    assert_eq!(m.silent_ms, 1000);
    assert!(!update_heartbeat(&mut m, 1, 100, &p));
}

#[test]
fn counter_advancing_again_clears_failure() {
    let p = SafetyParams::default();
    let mut m = HeartbeatMonitor::default();
    update_heartbeat(&mut m, 1, 100, &p);
    for _ in 0..12 {
        update_heartbeat(&mut m, 1, 100, &p);
    }
    assert!(!update_heartbeat(&mut m, 1, 100, &p));
    assert!(update_heartbeat(&mut m, 2, 100, &p));
    assert_eq!(m.silent_ms, 0);
}

#[test]
fn custom_timeout_is_honored() {
    let p = SafetyParams { heartbeat_timeout_ms: 300 };
    let mut m = HeartbeatMonitor::default();
    update_heartbeat(&mut m, 1, 100, &p);
    assert!(update_heartbeat(&mut m, 1, 100, &p));
    assert!(update_heartbeat(&mut m, 1, 100, &p));
    assert!(update_heartbeat(&mut m, 1, 100, &p)); // 300 ms
    assert!(!update_heartbeat(&mut m, 1, 100, &p)); // 400 ms
}

// ---- Supervisor (Safety SWC watchdog) ----

/// alive counters of [engine, brake, steering, vehicledynamics]
struct Rig {
    sup: SafetySupervisor,
    alive: [u64; 4],
    estop_req: bool,
}
impl Rig {
    fn new() -> Self {
        Self { sup: SafetySupervisor::new(), alive: [0; 4], estop_req: false }
    }
    /// One 100 ms cycle; each running SWC reports alive 10 times (10 ms tasks).
    fn cycle(&mut self, run: [bool; 4]) -> (bool, SafetyState) {
        for (a, r) in self.alive.iter_mut().zip(run) {
            if r {
                *a += 10;
            }
        }
        self.sup.main_100ms(self.alive, 100, self.estop_req)
    }
    fn cycles(&mut self, n: usize, run: [bool; 4]) -> (bool, SafetyState) {
        let mut last = (false, SafetyState::Normal);
        for _ in 0..n {
            last = self.cycle(run);
        }
        last
    }
}
const ALL: [bool; 4] = [true; 4];

#[test]
fn healthy_system_stays_normal() {
    let mut r = Rig::new();
    assert_eq!(r.cycles(200, ALL), (false, SafetyState::Normal));
}

#[test]
fn one_silent_swc_degrades_after_timeout_not_before() {
    let mut r = Rig::new();
    r.cycles(5, ALL);
    assert_eq!(r.cycles(10, [false, true, true, true]).1, SafetyState::Normal);
    assert_eq!(r.cycle([false, true, true, true]), (false, SafetyState::Degraded));
}

#[test]
fn degraded_recovers_when_swc_returns() {
    let mut r = Rig::new();
    assert_eq!(r.cycles(15, [true, false, true, true]).1, SafetyState::Degraded);
    assert_eq!(r.cycle(ALL).1, SafetyState::Normal);
}

#[test]
fn two_silent_swcs_estop_with_flag() {
    let mut r = Rig::new();
    assert_eq!(r.cycles(12, [false, false, true, true]), (true, SafetyState::EStop));
}

#[test]
fn watchdog_estop_is_latched_until_reset() {
    let mut r = Rig::new();
    assert!(r.cycles(12, [false, true, false, true]).0);
    assert_eq!(r.cycles(50, ALL), (true, SafetyState::EStop));
    r.estop_req = false; // external clear cannot release the latch
    assert_eq!(r.cycle(ALL), (true, SafetyState::EStop));
    r.sup.reset();
    assert_eq!(r.cycle(ALL), (false, SafetyState::Normal));
}

#[test]
fn external_estop_works_and_releases_without_latch() {
    let mut r = Rig::new();
    r.cycles(3, ALL);
    r.estop_req = true;
    assert_eq!(r.cycle(ALL), (true, SafetyState::EStop));
    r.estop_req = false;
    assert_eq!(r.cycle(ALL), (false, SafetyState::Normal));
}
