//! Config trial application (ADR-0003 §5): after an update the new config runs on probation; if the system goes
//! Degraded / EStop because of it, it is rolled back automatically.

use sdv_core::rte::SystemState;
use sdv_core::trial::{ConfigTrial, FailReason, TrialEvent, TrialState, DRIVER_ESTOP_GRACE_MS};

const NORMAL: SystemState = SystemState::Normal;

#[test]
fn a_new_trial_is_idle_and_does_nothing() {
    let mut t = ConfigTrial::default();
    assert_eq!(t.state(), TrialState::Idle);
    assert_eq!(
        t.tick(10, SystemState::EStop, false),
        TrialEvent::None,
        "no trial, nothing to judge"
    );
    assert_eq!(t.state(), TrialState::Idle);
}

#[test]
fn a_healthy_window_passes_exactly_when_it_has_elapsed() {
    let mut t = ConfigTrial::default();
    t.start(100);
    assert_eq!(
        t.state(),
        TrialState::Running {
            window_ms: 100,
            remaining_ms: 100
        }
    );
    for i in 1..10 {
        assert_eq!(t.tick(10, NORMAL, false), TrialEvent::None, "tick {i}");
    }
    assert_eq!(
        t.state(),
        TrialState::Running {
            window_ms: 100,
            remaining_ms: 10
        }
    );
    assert_eq!(t.tick(10, NORMAL, false), TrialEvent::Passed);
    assert_eq!(t.state(), TrialState::Passed);
    // a trial that has ended is over: later trouble is not this config's trial
    assert_eq!(t.tick(10, SystemState::EStop, false), TrialEvent::None);
    assert_eq!(t.state(), TrialState::Passed);
}

#[test]
fn degraded_and_estop_fail_the_trial_with_their_reason() {
    for (state, reason) in [
        (SystemState::Degraded, FailReason::Degraded),
        (SystemState::EStop, FailReason::EStop),
    ] {
        let mut t = ConfigTrial::default();
        t.start(5_000);
        assert_eq!(t.tick(10, NORMAL, false), TrialEvent::None);
        assert_eq!(t.tick(10, state, false), TrialEvent::Failed(reason));
        assert_eq!(t.state(), TrialState::RolledBack(reason));
    }
}

#[test]
fn a_failure_in_the_last_tick_still_fails_the_trial() {
    let mut t = ConfigTrial::default();
    t.start(20);
    assert_eq!(t.tick(10, NORMAL, false), TrialEvent::None);
    assert_eq!(
        t.tick(10, SystemState::Degraded, false),
        TrialEvent::Failed(FailReason::Degraded)
    );
}

#[test]
fn an_estop_the_driver_asked_for_is_not_the_configs_fault() {
    let mut t = ConfigTrial::default();
    t.start(50);
    // the driver pressed E-Stop: the system is in EStop, but not because of the config
    for _ in 0..4 {
        assert_eq!(t.tick(10, SystemState::EStop, true), TrialEvent::None);
    }
    // the time still counts down, and the trial passes if nothing else goes wrong
    assert_eq!(t.tick(10, NORMAL, false), TrialEvent::Passed);
}

#[test]
fn the_stale_estop_right_after_the_drivers_release_is_forgiven_for_one_safety_cycle() {
    // The Safety task runs every 100 ms, so for up to one cycle after the driver lets go the state still says EStop.
    assert_eq!(DRIVER_ESTOP_GRACE_MS, 100);
    let mut t = ConfigTrial::default();
    t.start(5_000);
    assert_eq!(
        t.tick(10, SystemState::EStop, true),
        TrialEvent::None,
        "driver request"
    );
    for i in 0..10 {
        assert_eq!(
            t.tick(10, SystemState::EStop, false),
            TrialEvent::None,
            "stale state, tick {i} after the release"
        );
    }
    assert_eq!(
        t.tick(10, SystemState::EStop, false),
        TrialEvent::Failed(FailReason::EStop),
        "it is the watchdog now"
    );
}

#[test]
fn a_new_driver_request_renews_the_grace() {
    let mut t = ConfigTrial::default();
    t.start(5_000);
    assert_eq!(t.tick(10, SystemState::EStop, true), TrialEvent::None);
    for _ in 0..9 {
        t.tick(10, SystemState::EStop, false);
    }
    assert_eq!(
        t.tick(10, SystemState::EStop, true),
        TrialEvent::None,
        "pressed again"
    );
    for i in 0..10 {
        assert_eq!(
            t.tick(10, SystemState::EStop, false),
            TrialEvent::None,
            "tick {i}"
        );
    }
    assert_eq!(
        t.tick(10, SystemState::EStop, false),
        TrialEvent::Failed(FailReason::EStop)
    );
}

#[test]
fn a_degraded_state_while_the_driver_holds_estop_is_not_judged_either() {
    let mut t = ConfigTrial::default();
    t.start(5_000);
    assert_eq!(t.tick(10, SystemState::Degraded, true), TrialEvent::None);
}

#[test]
fn start_restarts_the_trial_and_a_zero_window_is_refused() {
    let mut t = ConfigTrial::default();
    t.start(100);
    t.tick(10, NORMAL, false);
    t.start(200);
    assert_eq!(
        t.state(),
        TrialState::Running {
            window_ms: 200,
            remaining_ms: 200
        }
    );
    assert!(!t.try_start(0), "an empty window cannot judge anything");
    assert_eq!(
        t.state(),
        TrialState::Running {
            window_ms: 200,
            remaining_ms: 200
        },
        "a refused start changes nothing"
    );
    assert!(t.try_start(100));
}

#[test]
fn cancel_returns_to_idle() {
    let mut t = ConfigTrial::default();
    t.start(100);
    t.cancel();
    assert_eq!(t.state(), TrialState::Idle);
    assert_eq!(t.tick(10, SystemState::EStop, false), TrialEvent::None);
}

#[test]
fn codes_for_the_host_are_stable() {
    // The out buffer carries these numbers; the GUI and the integration tests depend on them.
    assert_eq!(TrialState::Idle.code(), 0);
    assert_eq!(
        TrialState::Running {
            window_ms: 1,
            remaining_ms: 1
        }
        .code(),
        1
    );
    assert_eq!(TrialState::Passed.code(), 2);
    assert_eq!(TrialState::RolledBack(FailReason::Degraded).code(), 3);
    assert_eq!(FailReason::Degraded.code(), 1);
    assert_eq!(FailReason::EStop.code(), 2);
}
