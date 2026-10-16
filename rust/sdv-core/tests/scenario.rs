//! Scenario (ADR-0003 step 5): driver inputs over time + timed events (faults, E-Stop) as JSON.
//! Reproducibility: the same scenario always yields the same log, and the JSON versions of the C++
//! fault-injection runs reproduce the C++ reference logs.

use sdv_core::bus::Node;
use sdv_core::scenario::{self, Scenario};
use sdv_core::sim::{Row, Sim, SystemState};

fn errs(json: &str) -> Vec<String> {
    match scenario::parse(json) {
        Ok(_) => vec![],
        Err(e) => e.into_iter().map(|e| e.path).collect(),
    }
}

fn run(json: &str, ticks: usize) -> Vec<Row> {
    let mut sim = Sim::new();
    sim.set_scenario(scenario::parse(json).expect("valid scenario"));
    sim.set_use_scenario(true);
    (0..ticks).map(|_| sim.step_10ms()).collect()
}

fn check_against(golden: &str, rows: &[Row]) {
    let mut lines = golden.lines();
    assert_eq!(lines.next().unwrap(), Row::CSV_HEADER);
    let g: Vec<&str> = lines.collect();
    assert_eq!(g.len(), rows.len());
    for (n, (gl, r)) in g.iter().zip(rows).enumerate() {
        let gv: Vec<f32> = gl.split(',').map(|s| s.parse().unwrap()).collect();
        let rv: Vec<f32> = r.to_csv().split(',').map(|s| s.parse().unwrap()).collect();
        for (k, (a, b)) in gv.iter().zip(&rv).enumerate() {
            let tol = match k { 7 | 8 => 3e-3, 9 => 2e-4, _ => 2e-3 };
            assert!((a - b).abs() <= tol, "row {n} col {k}: cpp={a} rust={b}\n  cpp : {gl}\n  rust: {}", r.to_csv());
        }
    }
}

// ---- format and validation ----

#[test]
fn the_demo_scenario_is_the_built_in_one() {
    let s = Scenario::demo();
    let at = |t: f32| {
        let i = s.input_at(t);
        (i.throttle, i.brake, i.steer)
    };
    assert_eq!(at(0.0), (0.6, 0.0, 0.0));
    assert_eq!(at(1.99), (0.6, 0.0, 0.0));
    assert_eq!(at(2.0), (0.2, 0.0, 0.2));
    assert_eq!(at(4.99), (0.2, 0.0, 0.2));
    assert_eq!(at(5.0), (0.0, 0.6, 0.0));
    assert_eq!(at(6.99), (0.0, 0.6, 0.0));
    assert_eq!(at(7.0), (0.0, 0.0, 0.0));
    assert_eq!(at(100.0), (0.0, 0.0, 0.0));
}

#[test]
fn json_round_trip_and_defaults() {
    let s = Scenario::demo();
    assert_eq!(scenario::parse(&scenario::to_json(&s)).unwrap(), s);
    let empty = scenario::parse("{}").unwrap();
    assert!(empty.steps.is_empty() && empty.events.is_empty());
    assert_eq!(empty.input_at(3.0).throttle, 0.0);
}

#[test]
fn unknown_fields_and_bad_json_are_rejected() {
    assert!(!errs(r#"{"stepz":[]}"#).is_empty());
    assert!(!errs(r#"{"steps":[{"until_s":1.0,"thrtle":0.5}]}"#).is_empty());
    assert!(!errs(r#"{"events":[{"at_s":1.0,"action":"explode"}]}"#).is_empty());
    assert!(!errs("{").is_empty());
}

#[test]
fn ranges_and_ordering_are_validated_with_paths() {
    let p = errs(r#"{"steps":[{"until_s":2.0,"throttle":1.5},{"until_s":1.0,"steer":-3.0}],"after":{"brake":2.0}}"#);
    for want in ["steps[0].throttle", "steps[1].until_s", "steps[1].steer", "after.brake"] {
        assert!(p.contains(&want.to_string()), "missing {want} in {p:?}");
    }
    assert_eq!(errs(r#"{"steps":[{"until_s":0.0}]}"#), vec!["steps[0].until_s"]);
    assert_eq!(errs(r#"{"schema":9}"#), vec!["schema"]);
}

#[test]
fn events_need_a_target_exactly_when_the_action_has_one() {
    assert_eq!(errs(r#"{"events":[{"at_s":1.0,"action":"stall"}]}"#), vec!["events[0].target"]);
    assert_eq!(errs(r#"{"events":[{"at_s":1.0,"action":"estop","target":"engine"}]}"#), vec!["events[0].target"]);
    assert!(errs(r#"{"events":[{"at_s":1.0,"action":"stall","target":"engine"},{"at_s":2.0,"action":"bus_down"}]}"#).is_empty());
    assert_eq!(errs(r#"{"events":[{"at_s":-1.0,"action":"estop"}]}"#), vec!["events[0].at_s"]);
}

#[test]
fn size_limits_are_enforced() {
    let steps: Vec<String> = (1..=65).map(|i| format!(r#"{{"until_s":{i}.0}}"#)).collect();
    assert_eq!(errs(&format!(r#"{{"steps":[{}]}}"#, steps.join(","))), vec!["steps"]);
}

// ---- running a scenario ----

#[test]
fn a_custom_scenario_drives_the_inputs() {
    let rows = run(r#"{"steps":[{"until_s":1.0,"throttle":1.0},{"until_s":2.0,"brake":1.0}]}"#, 300);
    let at = |i: usize| rows[i].input;
    assert_eq!(at(50).throttle, 1.0);
    assert_eq!(at(150).brake, 1.0);
    assert_eq!(at(150).throttle, 0.0);
    assert!(rows[99].state.v > 1.5, "accelerating during the first second");
    assert!(rows[299].state.v < 0.01, "braking afterwards stops the car");
}

#[test]
fn the_same_scenario_always_gives_the_same_log() {
    let json = r#"{"steps":[{"until_s":3.0,"throttle":0.7,"steer":0.3}],
                   "events":[{"at_s":1.5,"action":"stall","target":"brake"},{"at_s":2.0,"action":"bus_down"}]}"#;
    let a: Vec<String> = run(json, 600).iter().map(Row::to_csv).collect();
    let b: Vec<String> = run(json, 600).iter().map(Row::to_csv).collect();
    assert_eq!(a, b);
}

#[test]
fn events_fire_at_their_time_in_order() {
    let json = r#"{"steps":[{"until_s":10.0,"throttle":1.0}],
        "events":[{"at_s":2.0,"action":"estop"},{"at_s":3.0,"action":"estop_release"}]}"#;
    let mut sim = Sim::new();
    sim.set_scenario(scenario::parse(json).unwrap());
    sim.set_use_scenario(true);
    let mut seen = Vec::new();
    for _ in 0..500 {
        sim.step_10ms();
        seen.push(sim.safety().system_state);
    }
    assert_eq!(seen[100], SystemState::Normal);
    assert_eq!(seen[230], SystemState::EStop);
    assert_eq!(seen[450], SystemState::Normal);
}

#[test]
fn bus_and_vcu_events_cause_the_ecu_failsafe() {
    let mut sim = Sim::new();
    sim.set_scenario(
        scenario::parse(r#"{"steps":[{"until_s":10.0,"throttle":1.0}],"events":[{"at_s":1.0,"action":"bus_down"},{"at_s":2.0,"action":"bus_up"}]}"#).unwrap(),
    );
    sim.set_use_scenario(true);
    let mut during = false;
    let mut after = true;
    for t in 0..400 {
        sim.step_10ms();
        if t == 150 {
            during = sim.ecu_failsafe(Node::Engine).safety_timeout;
        }
        if t == 399 {
            after = sim.ecu_failsafe(Node::Engine).safety_timeout;
        }
    }
    assert!(during && !after);
}

#[test]
fn reset_replays_the_scenario_including_its_events() {
    let json = r#"{"steps":[{"until_s":10.0,"throttle":0.5}],"events":[{"at_s":1.0,"action":"estop"}]}"#;
    let mut sim = Sim::new();
    sim.set_scenario(scenario::parse(json).unwrap());
    sim.set_use_scenario(true);
    let first: Vec<String> = (0..300).map(|_| sim.step_10ms().to_csv()).collect();
    sim.reset();
    let second: Vec<String> = (0..300).map(|_| sim.step_10ms().to_csv()).collect();
    assert_eq!(first, second);
    assert_eq!(sim.safety().system_state, SystemState::EStop);
}

// ---- the C++ reference runs, expressed as scenario JSON ----

const DEMO_STEPS: &str = r#""steps":[
    {"until_s":2.0,"throttle":0.6},
    {"until_s":5.0,"throttle":0.2,"steer":0.2},
    {"until_s":7.0,"brake":0.6}]"#;

#[test]
fn demo_scenario_json_reproduces_the_cpp_log() {
    let rows = run(&format!("{{{DEMO_STEPS}}}"), 1000);
    check_against(include_str!("golden/cpp_latest.csv"), &rows);
}

#[test]
fn json_events_reproduce_the_cpp_fault_injection_logs() {
    let cases = [
        (r#"{"action":"stall","target":"brake","at_s":2.0}"#, include_str!("golden/cpp_stall_brake2.csv")),
        (
            r#"{"action":"stall","target":"steering","at_s":3.0},{"action":"stall","target":"brake","at_s":3.5}"#,
            include_str!("golden/cpp_stall_steering3_brake3.5.csv"),
        ),
        (
            r#"{"action":"stall","target":"engine","at_s":1.0},{"action":"stall","target":"vehicledynamics","at_s":2.0}"#,
            include_str!("golden/cpp_stall_engine1_vd2.csv"),
        ),
    ];
    for (events, golden) in cases {
        let rows = run(&format!(r#"{{{DEMO_STEPS},"events":[{events}]}}"#), 1000);
        check_against(golden, &rows);
    }
}
