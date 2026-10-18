//! Integration tests: the declarative scenario cases in `tests/integration/scenarios/*.json` (repo root), driven through
//! the same C ABI the browser uses. The Node runner (`tests/integration/run_wasm.mjs`) evaluates the same files against
//! the built `.wasm`, so a native/wasm difference shows up as one passing and one failing.
//! Global state in the C ABI => one sequential test per binary.

use sdv_core::*;
use serde_json::Value;
use std::path::PathBuf;

/// Layout of the output buffer; must equal `SIGNALS` in `tests/integration/lib.mjs` (checked below).
const SIGNALS: [&str; 28] = [
    "t", "x", "y", "yaw", "v", "yaw_rate", "wheel_omega", "throttle", "brake", "steer",
    "drive_accel_cmd", "brake_decel_cmd", "steer_angle_cmd", "estop", "state",
    "silent_engine_ms", "silent_brake_ms", "silent_steering_ms", "silent_plant_ms",
    "failsafe_engine", "failsafe_brake", "failsafe_steering",
    "stalled_engine", "stalled_brake", "stalled_steering", "stalled_plant", "stalled_vcu", "bus_down",
];
const EPS: f64 = 1e-4;
const HALF_TICK: f64 = 0.005;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/integration")
}

fn with_text<R>(s: &str, f: impl FnOnce(*const u8, i32) -> R) -> R {
    let p = sdv_alloc(s.len() as i32);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len()) };
    let r = f(p, s.len() as i32);
    sdv_free(p, s.len() as i32);
    r
}

fn buf() -> String {
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(sdv_buf_ptr(), sdv_buf_len() as usize) }).to_string()
}

fn run(scenario: &Value, seconds: f64, config: Option<&Value>) -> Vec<Vec<f64>> {
    sdv_nvm_erase(0);
    sdv_nvm_erase(1);
    if let Some(c) = config {
        let r = with_text(&c.to_string(), |p, n| sdv_nvm_commit(p, n));
        assert_eq!(r, 0, "config rejected: {}", buf());
    }
    sdv_boot();
    let r = with_text(&scenario.to_string(), |p, n| sdv_set_scenario(p, n));
    assert_eq!(r, 0, "scenario rejected: {}", buf());
    sdv_use_scenario(1);
    sdv_reset();
    (0..(seconds * 100.0).round() as usize)
        .map(|_| {
            sdv_step(1);
            unsafe { std::slice::from_raw_parts(sdv_out_ptr(), sdv_out_len() as usize) }.iter().map(|&x| x as f64).collect()
        })
        .collect()
}

fn sample(row: &[f64], signal: &str) -> f64 {
    if signal == "lateral_accel" {
        return row[4] * row[5]; // v * yaw_rate
    }
    row[SIGNALS.iter().position(|s| *s == signal).unwrap_or_else(|| panic!("unknown signal '{signal}'"))]
}

fn holds(row: &[f64], c: &Value) -> bool {
    let (a, v) = (sample(row, c["signal"].as_str().unwrap()), c["value"].as_f64().unwrap());
    match c["op"].as_str().unwrap() {
        "==" => (a - v).abs() <= EPS,
        "!=" => (a - v).abs() > EPS,
        "<" => a < v,
        "<=" => a <= v + EPS,
        ">" => a > v,
        ">=" => a >= v - EPS,
        "approx" => (a - v).abs() <= c["tol"].as_f64().unwrap(),
        op => panic!("unknown op '{op}'"),
    }
}

fn describe(c: &Value) -> String {
    format!("{} {} {}", c["signal"].as_str().unwrap(), c["op"].as_str().unwrap(), c["value"])
}

fn evaluate(rows: &[Vec<f64>], checks: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    let f = |c: &Value, k: &str| c[k].as_f64().unwrap();
    for c in checks {
        match c["kind"].as_str().unwrap() {
            "at" => match rows.iter().find(|r| r[0] >= f(c, "t") - HALF_TICK) {
                None => failures.push(format!("at {}s: the run ended before this time", c["t"])),
                Some(r) if !holds(r, c) => failures.push(format!(
                    "at {}s: expected {}, got {}",
                    c["t"],
                    describe(c),
                    sample(r, c["signal"].as_str().unwrap())
                )),
                _ => {}
            },
            "always" => {
                let (from, to) = (f(c, "from"), f(c, "to"));
                if let Some(bad) = rows.iter().find(|r| r[0] >= from - HALF_TICK && r[0] <= to + HALF_TICK && !holds(r, c)) {
                    failures.push(format!("always [{from}, {to}]s: {} violated at t={:.2}", describe(c), bad[0]));
                }
                if !rows.iter().any(|r| r[0] >= to - HALF_TICK) {
                    failures.push(format!("always [{from}, {to}]s: the run ended before {to}s"));
                }
            }
            "first" => {
                let (lo, hi) = (c["between"][0].as_f64().unwrap(), c["between"][1].as_f64().unwrap());
                match rows.iter().find(|r| holds(r, c)) {
                    None => failures.push(format!("first: {} never became true", describe(c))),
                    Some(r) if r[0] < lo - HALF_TICK || r[0] > hi + HALF_TICK => failures
                        .push(format!("first: {} became true at t={:.2}, expected within [{lo}, {hi}]", describe(c), r[0])),
                    _ => {}
                }
            }
            k => failures.push(format!("unknown check kind '{k}'")),
        }
    }
    failures
}

#[test]
fn signal_names_match_the_node_runner() {
    let js = std::fs::read_to_string(root().join("lib.mjs")).unwrap();
    let list = &js[js.find("SIGNALS = [").unwrap()..];
    let list = &list[..list.find("];").unwrap()];
    let names: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
    assert_eq!(names, SIGNALS, "SIGNALS in lib.mjs and in this file must be the same list");
}

#[test]
fn scenario_cases_meet_their_expectations() {
    let mut files: Vec<_> = std::fs::read_dir(root().join("scenarios"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(files.len() >= 9, "expected the scenario cases to be found, got {files:?}");

    let mut all = Vec::new();
    for file in &files {
        let case: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let rows = run(&case["scenario"], case["seconds"].as_f64().unwrap(), case.get("config"));
        for failure in evaluate(&rows, case["expect"].as_array().unwrap()) {
            all.push(format!("{}: {failure}", case["name"].as_str().unwrap()));
        }
    }
    assert!(all.is_empty(), "\n{}", all.join("\n"));
}
