//! Config (ADR-0003): JSON, defaults for missing fields, unknown fields rejected,
//! range validation with error paths, and the config actually changing behaviour.

use sdv_core::bus::Node;
use sdv_core::config::{self, Config};
use sdv_core::sim::{Component, Sim, SystemState};

fn errs(json: &str) -> Vec<(String, String)> {
    match config::parse(json) {
        Ok(_) => vec![],
        Err(e) => e.into_iter().map(|e| (e.path, e.reason)).collect(),
    }
}

fn paths(json: &str) -> Vec<String> {
    errs(json).into_iter().map(|(p, _)| p).collect()
}

#[test]
fn empty_object_gives_the_defaults() {
    assert_eq!(config::parse("{}").unwrap(), Config::default());
}

#[test]
fn json_round_trip_keeps_every_value() {
    let c = Config::default();
    assert_eq!(config::parse(&config::to_json(&c)).unwrap(), c);
}

#[test]
fn missing_fields_fall_back_to_defaults() {
    let c = config::parse(r#"{"engine":{"max_accel_mps2":4.0}}"#).unwrap();
    assert_eq!(c.engine.max_accel_mps2, 4.0);
    assert_eq!(c.brake, Config::default().brake);
    assert_eq!(c.safety, Config::default().safety);
}

#[test]
fn unknown_fields_are_rejected_at_every_level() {
    assert!(!errs(r#"{"engnie":{}}"#).is_empty());
    assert!(!errs(r#"{"engine":{"max_acel_mps2":1.0}}"#).is_empty());
}

#[test]
fn broken_json_and_wrong_types_are_rejected() {
    assert!(!errs("{").is_empty());
    assert!(!errs("").is_empty());
    assert!(!errs(r#"{"engine":{"max_accel_mps2":"fast"}}"#).is_empty());
    assert!(!errs("[1,2]").is_empty());
}

#[test]
fn unsupported_schema_is_rejected() {
    assert_eq!(paths(r#"{"schema":2}"#), vec!["schema"]);
    assert!(errs(r#"{"schema":1}"#).is_empty());
}

#[test]
fn every_range_violation_is_reported_with_its_path() {
    let json = r#"{
        "engine":{"max_accel_mps2":-1.0},
        "steering":{"max_steer_angle_rad":1.0,"time_constant_s":0.0},
        "vehicle":{"wheel_radius_m":0.0},
        "safety":{"heartbeat_timeout_ms":50},
        "failsafe":{"driver_timeout_ms":10,"safety_timeout_ms":5000}
    }"#;
    let p = paths(json);
    for want in [
        "engine.max_accel_mps2",
        "steering.max_steer_angle_rad",
        "steering.time_constant_s",
        "vehicle.wheel_radius_m",
        "safety.heartbeat_timeout_ms",
        "failsafe.driver_timeout_ms",
        "failsafe.safety_timeout_ms",
    ] {
        assert!(p.contains(&want.to_string()), "missing {want} in {p:?}");
    }
}

#[test]
fn estop_decel_must_not_be_weaker_than_normal_braking() {
    let p = paths(r#"{"brake":{"max_decel_mps2":6.0,"estop_max_decel_mps2":4.0}}"#);
    assert_eq!(p, vec!["brake.estop_max_decel_mps2"]);
}

#[test]
fn range_boundaries_are_accepted() {
    for json in [
        r#"{"engine":{"max_accel_mps2":0.0}}"#,
        r#"{"engine":{"max_accel_mps2":10.0}}"#,
        r#"{"steering":{"max_steer_angle_rad":0.05}}"#,
        r#"{"steering":{"max_steer_angle_rad":0.6}}"#,
        r#"{"safety":{"heartbeat_timeout_ms":200}}"#,
        r#"{"safety":{"heartbeat_timeout_ms":10000}}"#,
        r#"{"failsafe":{"driver_timeout_ms":50,"safety_timeout_ms":30}}"#,
    ] {
        assert!(errs(json).is_empty(), "{json}: {:?}", errs(json));
    }
    assert!(!errs(r#"{"engine":{"max_accel_mps2":10.01}}"#).is_empty());
    assert!(!errs(r#"{"steering":{"max_steer_angle_rad":0.04}}"#).is_empty());
}

#[test]
fn failsafe_milliseconds_become_ticks() {
    let c = Config::default();
    let f = c.failsafe_params();
    assert_eq!((f.driver_timeout_ticks, f.safety_timeout_ticks), (20, 10));
    let c = config::parse(r#"{"failsafe":{"driver_timeout_ms":55,"safety_timeout_ms":35}}"#).unwrap();
    let f = c.failsafe_params();
    assert_eq!((f.driver_timeout_ticks, f.safety_timeout_ticks), (6, 4), "rounded up to whole ticks");
}

// ---- the config changes behaviour ----

fn speed_after(cfg: &Config, ticks: usize) -> f32 {
    let mut s = Sim::with_config(cfg.clone());
    s.input_mut().throttle = 1.0;
    for _ in 0..ticks {
        s.step_10ms();
    }
    s.state().v
}

#[test]
fn engine_config_changes_acceleration() {
    let weak = config::parse(r#"{"engine":{"max_accel_mps2":1.0}}"#).unwrap();
    let strong = config::parse(r#"{"engine":{"max_accel_mps2":4.0}}"#).unwrap();
    assert!(speed_after(&strong, 50) > speed_after(&weak, 50) * 2.0);
}

#[test]
fn vehicle_config_changes_the_speed_limit() {
    let c = config::parse(r#"{"vehicle":{"max_speed_mps":1.0}}"#).unwrap();
    assert!((speed_after(&c, 1000) - 1.0).abs() < 1e-4);
}

#[test]
fn steering_config_changes_the_turn_rate() {
    let yaw_rate = |cfg: &Config| {
        let mut s = Sim::with_config(cfg.clone());
        s.input_mut().throttle = 0.6;
        s.input_mut().steer = 1.0;
        for _ in 0..300 {
            s.step_10ms();
        }
        s.state().yaw_rate.abs()
    };
    let small = config::parse(r#"{"steering":{"max_steer_angle_rad":0.1}}"#).unwrap();
    assert!(yaw_rate(&Config::default()) > yaw_rate(&small) * 3.0);
}

#[test]
fn watchdog_timeout_config_changes_detection_time() {
    let first_degraded = |cfg: &Config| {
        let mut s = Sim::with_config(cfg.clone());
        s.set_stall(Component::Steering, true);
        for t in 0..500 {
            s.step_10ms();
            if s.safety().system_state == SystemState::Degraded {
                return t;
            }
        }
        panic!("never degraded");
    };
    let fast = config::parse(r#"{"safety":{"heartbeat_timeout_ms":300}}"#).unwrap();
    assert!(first_degraded(&fast) + 50 < first_degraded(&Config::default()));
}

#[test]
fn failsafe_config_changes_the_ecu_reaction_time() {
    let ticks_until_failsafe = |cfg: &Config| {
        let mut s = Sim::with_config(cfg.clone());
        for _ in 0..50 {
            s.step_10ms();
        }
        s.set_bus_down(true);
        for t in 0..200 {
            s.step_10ms();
            if s.ecu_failsafe(Node::Engine).safety_timeout {
                return t;
            }
        }
        panic!("no failsafe");
    };
    let slow = config::parse(r#"{"failsafe":{"safety_timeout_ms":300}}"#).unwrap();
    assert!(ticks_until_failsafe(&slow) >= ticks_until_failsafe(&Config::default()) + 15);
}

#[test]
fn set_config_restarts_the_sim_and_keeps_the_scenario_flag() {
    let mut s = Sim::new();
    s.set_use_scenario(true);
    for _ in 0..300 {
        s.step_10ms();
    }
    assert!(s.state().t > 2.0);
    s.set_config(config::parse(r#"{"engine":{"max_accel_mps2":4.0}}"#).unwrap());
    assert_eq!(s.state().t, 0.0, "restarted");
    assert_eq!(s.config().engine.max_accel_mps2, 4.0);
    for _ in 0..10 {
        s.step_10ms();
    }
    assert!(s.input().throttle > 0.0, "scenario still active");
    s.reset();
    assert_eq!(s.config().engine.max_accel_mps2, 4.0, "reset keeps the config");
}
