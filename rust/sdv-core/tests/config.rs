//! Config schema 2 (ADR-0003 revision 1): vehicle_spec / physics / tuning (+ derived values),
//! automatic conversion of schema 1, validation with error paths, and the config changing behaviour.

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

// ---- schema 2: format ----

#[test]
fn empty_object_gives_the_defaults() {
    let c = config::parse("{}").unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.schema, 2);
}

#[test]
fn json_round_trip_keeps_every_value_and_writes_schema_2() {
    let c = Config::default();
    let json = config::to_json(&c);
    assert!(json.contains("\"schema\": 2") && json.contains("vehicle_spec") && json.contains("physics") && json.contains("tuning"));
    assert_eq!(config::parse(&json).unwrap(), c);
}

#[test]
fn missing_fields_fall_back_to_defaults() {
    let c = config::parse(r#"{"vehicle_spec":{"max_accel_mps2":4.0}}"#).unwrap();
    assert_eq!(c.vehicle_spec.max_accel_mps2, 4.0);
    assert_eq!(c.vehicle_spec.wheelbase_m, Config::default().vehicle_spec.wheelbase_m);
    assert_eq!(c.physics, Config::default().physics);
    assert_eq!(c.tuning, Config::default().tuning);
}

#[test]
fn unknown_fields_are_rejected_at_every_level() {
    assert!(!errs(r#"{"vehicle_specs":{}}"#).is_empty());
    assert!(!errs(r#"{"vehicle_spec":{"wheelbase":0.2}}"#).is_empty());
    assert!(!errs(r#"{"physics":{"mu":0.6}}"#).is_empty());
    assert!(!errs(r#"{"tuning":{"safety":{"timeout":1}}}"#).is_empty());
}

#[test]
fn broken_json_and_wrong_types_are_rejected() {
    assert!(!errs("{").is_empty());
    assert!(!errs("").is_empty());
    assert!(!errs(r#"{"vehicle_spec":{"max_accel_mps2":"fast"}}"#).is_empty());
    assert!(!errs("[1,2]").is_empty());
}

#[test]
fn unsupported_schema_is_rejected() {
    assert_eq!(paths(r#"{"schema":3}"#), vec!["schema"]);
    assert_eq!(paths(r#"{"schema":0}"#), vec!["schema"]);
    assert!(errs(r#"{"schema":2}"#).is_empty());
}

#[test]
fn every_range_violation_is_reported_with_its_v2_path() {
    let json = r#"{
        "vehicle_spec":{"max_accel_mps2":-1.0,"max_steer_angle_rad":1.0,"steer_time_constant_s":0.0,
                        "wheel_radius_m":0.0,"top_speed_mps":0.0},
        "physics":{"gravity_mps2":0.0,"tire_friction_mu":3.0,"drag_coeff":-1.0},
        "tuning":{"safety":{"heartbeat_timeout_ms":50},"failsafe":{"driver_timeout_ms":10,"safety_timeout_ms":5000}}
    }"#;
    let p = paths(json);
    for want in [
        "vehicle_spec.max_accel_mps2", "vehicle_spec.max_steer_angle_rad", "vehicle_spec.steer_time_constant_s",
        "vehicle_spec.wheel_radius_m", "vehicle_spec.top_speed_mps",
        "physics.gravity_mps2", "physics.tire_friction_mu", "physics.drag_coeff",
        "tuning.safety.heartbeat_timeout_ms", "tuning.failsafe.driver_timeout_ms", "tuning.failsafe.safety_timeout_ms",
    ] {
        assert!(p.contains(&want.to_string()), "missing {want} in {p:?}");
    }
}

#[test]
fn estop_decel_must_not_be_weaker_than_normal_braking() {
    let p = paths(r#"{"vehicle_spec":{"max_decel_mps2":6.0},"tuning":{"estop_max_decel_mps2":4.0}}"#);
    assert_eq!(p, vec!["tuning.estop_max_decel_mps2"]);
}

#[test]
fn range_boundaries_are_accepted() {
    for json in [
        r#"{"vehicle_spec":{"max_accel_mps2":0.0}}"#,
        r#"{"vehicle_spec":{"max_accel_mps2":10.0}}"#,
        r#"{"vehicle_spec":{"max_steer_angle_rad":0.05}}"#,
        r#"{"vehicle_spec":{"max_steer_angle_rad":0.6}}"#,
        r#"{"physics":{"tire_friction_mu":0.05}}"#,
        r#"{"physics":{"tire_friction_mu":1.5}}"#,
        r#"{"physics":{"gravity_mps2":1.0}}"#,
        r#"{"physics":{"gravity_mps2":25.0}}"#,
        r#"{"tuning":{"safety":{"heartbeat_timeout_ms":200}}}"#,
        r#"{"tuning":{"safety":{"heartbeat_timeout_ms":10000}}}"#,
        r#"{"tuning":{"failsafe":{"driver_timeout_ms":50,"safety_timeout_ms":30}}}"#,
    ] {
        assert!(errs(json).is_empty(), "{json}: {:?}", errs(json));
    }
    assert!(!errs(r#"{"vehicle_spec":{"max_accel_mps2":10.01}}"#).is_empty());
    assert!(!errs(r#"{"physics":{"tire_friction_mu":1.51}}"#).is_empty());
    assert!(!errs(r#"{"physics":{"tire_friction_mu":0.04}}"#).is_empty());
}

#[test]
fn failsafe_milliseconds_become_ticks() {
    let f = Config::default().failsafe_params();
    assert_eq!((f.driver_timeout_ticks, f.safety_timeout_ticks), (20, 10));
    let c = config::parse(r#"{"tuning":{"failsafe":{"driver_timeout_ms":55,"safety_timeout_ms":35}}}"#).unwrap();
    let f = c.failsafe_params();
    assert_eq!((f.driver_timeout_ticks, f.safety_timeout_ticks), (6, 4), "rounded up to whole ticks");
}

// ---- schema 1 is converted automatically ----

#[test]
fn schema_1_documents_are_converted_to_schema_2() {
    let v1 = r#"{
        "schema": 1,
        "engine":   {"max_accel_mps2": 3.0},
        "brake":    {"max_decel_mps2": 5.0, "estop_max_decel_mps2": 7.0},
        "steering": {"max_steer_angle_rad": 0.35, "time_constant_s": 0.2},
        "vehicle":  {"wheel_radius_m": 0.04, "wheelbase_m": 0.25, "linear_drag": 0.3,
                     "max_speed_mps": 4.0, "estop_decel_mps2": 8.0},
        "safety":   {"heartbeat_timeout_ms": 1500},
        "failsafe": {"driver_timeout_ms": 300, "safety_timeout_ms": 150}
    }"#;
    let c = config::parse(v1).unwrap();
    assert_eq!(c.schema, 2);
    let (s, p, t) = (&c.vehicle_spec, &c.physics, &c.tuning);
    assert_eq!((s.max_accel_mps2, s.max_decel_mps2, t.estop_max_decel_mps2), (3.0, 5.0, 7.0));
    assert_eq!((s.max_steer_angle_rad, s.steer_time_constant_s), (0.35, 0.2));
    assert_eq!((s.wheel_radius_m, s.wheelbase_m, s.top_speed_mps, s.estop_decel_mps2), (0.04, 0.25, 4.0, 8.0));
    assert_eq!(p.drag_coeff, 0.3);
    assert_eq!(t.safety.heartbeat_timeout_ms, 1500);
    assert_eq!((t.failsafe.driver_timeout_ms, t.failsafe.safety_timeout_ms), (300, 150));
    // new schema 2 fields take their defaults
    assert_eq!(p.gravity_mps2, Config::default().physics.gravity_mps2);
    assert_eq!(p.tire_friction_mu, Config::default().physics.tire_friction_mu);
}

#[test]
fn schema_1_without_the_schema_key_is_recognised_by_its_keys() {
    let c = config::parse(r#"{"engine":{"max_accel_mps2":4.0}}"#).unwrap();
    assert_eq!(c.vehicle_spec.max_accel_mps2, 4.0);
    assert_eq!(c.physics, Config::default().physics);
}

#[test]
fn an_empty_schema_1_document_is_the_default_config() {
    assert_eq!(config::parse(r#"{"schema":1}"#).unwrap(), Config::default());
}

#[test]
fn schema_1_is_still_validated_and_unknown_keys_are_still_rejected() {
    assert!(!errs(r#"{"schema":1,"engnie":{}}"#).is_empty());
    assert!(!errs(r#"{"engine":{"max_acel_mps2":1.0}}"#).is_empty());
    // range errors are reported with the schema 2 path of the converted value
    assert_eq!(paths(r#"{"engine":{"max_accel_mps2":99.0}}"#), vec!["vehicle_spec.max_accel_mps2"]);
    assert_eq!(
        paths(r#"{"brake":{"max_decel_mps2":6.0,"estop_max_decel_mps2":4.0}}"#),
        vec!["tuning.estop_max_decel_mps2"]
    );
}

#[test]
fn mixing_schema_1_keys_with_schema_2_keys_is_rejected() {
    assert!(!errs(r#"{"schema":2,"engine":{"max_accel_mps2":3.0}}"#).is_empty());
    assert!(!errs(r#"{"engine":{"max_accel_mps2":3.0},"physics":{"tire_friction_mu":0.5}}"#).is_empty());
}

// ---- derived values ----

#[test]
fn derived_values_follow_from_spec_and_physics() {
    let d = Config::default().derived();
    assert!((d.min_turn_radius_m - 0.20 / 0.40f32.tan()).abs() < 1e-5);
    assert!((d.max_lateral_accel_mps2 - 0.6 * 9.81).abs() < 1e-5);
    assert!((d.critical_speed_mps - (d.max_lateral_accel_mps2 * d.min_turn_radius_m).sqrt()).abs() < 1e-5);
}

#[test]
fn derived_values_change_with_the_car_and_the_surface() {
    let base = Config::default().derived();
    let long = config::parse(r#"{"vehicle_spec":{"wheelbase_m":0.40}}"#).unwrap().derived();
    assert!((long.min_turn_radius_m - 2.0 * base.min_turn_radius_m).abs() < 1e-5);
    let grippy = config::parse(r#"{"physics":{"tire_friction_mu":1.2}}"#).unwrap().derived();
    assert!((grippy.max_lateral_accel_mps2 - 2.0 * base.max_lateral_accel_mps2).abs() < 1e-4);
}

#[test]
fn the_derived_turn_curve_matches_the_formulas() {
    let c = Config::default();
    assert!((c.max_steer_at_speed(0.0) - 0.40).abs() < 1e-6);
    assert!((c.max_steer_at_speed(1.0) - 0.40).abs() < 1e-6);
    let at3 = c.max_steer_at_speed(3.0);
    assert!((at3 - (0.20 * 0.6 * 9.81f32 / 9.0).atan()).abs() < 1e-5);
    assert!((c.yaw_rate_at_full_lock(3.0) - 0.6 * 9.81 / 3.0).abs() < 1e-4);
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
fn engine_spec_changes_acceleration() {
    let weak = config::parse(r#"{"vehicle_spec":{"max_accel_mps2":1.0}}"#).unwrap();
    let strong = config::parse(r#"{"vehicle_spec":{"max_accel_mps2":4.0}}"#).unwrap();
    assert!(speed_after(&strong, 50) > speed_after(&weak, 50) * 2.0);
}

#[test]
fn top_speed_spec_changes_the_speed_limit() {
    let c = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0}}"#).unwrap();
    assert!((speed_after(&c, 1000) - 1.0).abs() < 1e-4);
}

fn full_lock_yaw_rate(cfg: &Config, throttle: f32, ticks: usize) -> f32 {
    let mut s = Sim::with_config(cfg.clone());
    s.input_mut().throttle = throttle;
    s.input_mut().steer = 1.0;
    for _ in 0..ticks {
        s.step_10ms();
    }
    s.state().yaw_rate.abs()
}

#[test]
fn steering_spec_changes_the_turn_rate() {
    // slow (below the critical speed) so the grip limit does not interfere
    let normal = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0}}"#).unwrap();
    let small = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0,"max_steer_angle_rad":0.1}}"#).unwrap();
    assert!(full_lock_yaw_rate(&normal, 1.0, 800) > full_lock_yaw_rate(&small, 1.0, 800) * 3.0);
}

#[test]
fn tyre_friction_limits_the_turn_at_speed() {
    // at the 3 m/s top speed full lock would give 6.3 rad/s without a grip limit
    let normal = full_lock_yaw_rate(&Config::default(), 1.0, 1000);
    assert!(normal < 2.1, "grip-limited, got {normal}");
    let grippy = config::parse(r#"{"physics":{"tire_friction_mu":1.5}}"#).unwrap();
    let slippery = config::parse(r#"{"physics":{"tire_friction_mu":0.2}}"#).unwrap();
    assert!(full_lock_yaw_rate(&grippy, 1.0, 1000) > normal * 1.5);
    assert!(full_lock_yaw_rate(&slippery, 1.0, 1000) < normal * 0.6);
}

#[test]
fn low_speed_turning_is_not_affected_by_the_friction() {
    // slow: stays below the critical speed (1.67 m/s) the whole time
    let slow = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0}}"#).unwrap();
    let slippery = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0},"physics":{"tire_friction_mu":0.3}}"#).unwrap();
    let a = full_lock_yaw_rate(&slow, 1.0, 800);
    assert!((a - full_lock_yaw_rate(&slippery, 1.0, 800)).abs() < 1e-5);
    assert!((a - (1.0 / 0.20) * 0.40f32.tan()).abs() < 0.05);
}

#[test]
fn the_physical_turn_radius_follows_the_wheelbase() {
    // a longer car turns wider at the same speed and steering
    let long = config::parse(r#"{"vehicle_spec":{"wheelbase_m":0.40,"top_speed_mps":1.0}}"#).unwrap();
    let short = config::parse(r#"{"vehicle_spec":{"top_speed_mps":1.0}}"#).unwrap();
    assert!(full_lock_yaw_rate(&short, 1.0, 800) > full_lock_yaw_rate(&long, 1.0, 800) * 1.8);
}

#[test]
fn watchdog_timeout_tuning_changes_detection_time() {
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
    let fast = config::parse(r#"{"tuning":{"safety":{"heartbeat_timeout_ms":300}}}"#).unwrap();
    assert!(first_degraded(&fast) + 50 < first_degraded(&Config::default()));
}

#[test]
fn failsafe_tuning_changes_the_ecu_reaction_time() {
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
    let slow = config::parse(r#"{"tuning":{"failsafe":{"safety_timeout_ms":300}}}"#).unwrap();
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
    s.set_config(config::parse(r#"{"vehicle_spec":{"max_accel_mps2":4.0}}"#).unwrap());
    assert_eq!(s.state().t, 0.0, "restarted");
    assert_eq!(s.config().vehicle_spec.max_accel_mps2, 4.0);
    for _ in 0..10 {
        s.step_10ms();
    }
    assert!(s.input().throttle > 0.0, "scenario still active");
    s.reset();
    assert_eq!(s.config().vehicle_spec.max_accel_mps2, 4.0, "reset keeps the config");
}
