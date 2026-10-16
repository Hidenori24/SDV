//! Cross-check: the Rust port must reproduce the C++ reference log
//! (`tests/golden/cpp_latest.csv`, produced by `build/sdv_sim`).

use sdv_core::sim::Sim;

fn field(s: &str) -> f32 {
    s.parse().unwrap()
}

#[test]
fn rust_matches_cpp_reference_log() {
    let golden = include_str!("golden/cpp_latest.csv");
    let mut lines = golden.lines();
    let header = lines.next().unwrap();
    assert_eq!(header, sdv_core::sim::Row::CSV_HEADER);

    let mut sim = Sim::new();
    sim.use_scenario = true;

    let mut rows = 0;
    for line in lines {
        let g: Vec<f32> = line.split(',').map(field).collect();
        let r = sim.step_10ms();
        let mine: Vec<f32> = r.to_csv().split(',').map(field).collect();
        // x/y are printed with 3 decimals; allow a couple of ulps of libm difference.
        for (k, (a, b)) in g.iter().zip(mine.iter()).enumerate() {
            let tol = match k { 7 | 8 => 3e-3, 9 => 2e-4, _ => 2e-3 };
            assert!((a - b).abs() <= tol, "row {rows} col {k}: cpp={a} rust={b}\n  cpp : {line}\n  rust: {}", r.to_csv());
        }
        rows += 1;
    }
    assert_eq!(rows, 1000);
}

#[test]
fn estop_stops_the_vehicle() {
    let mut sim = Sim::new();
    sim.input.throttle = 1.0;
    for _ in 0..300 { sim.step_10ms(); }
    assert!(sim.state.v > 0.5);
    sim.safety.estop = true;
    for _ in 0..200 { sim.step_10ms(); }
    assert_eq!(sim.state.v, 0.0);
    assert_eq!(sim.safety.system_state, sdv_core::sim::SystemState::EStop);
}

#[test]
fn speed_is_capped_and_never_negative() {
    let mut sim = Sim::new();
    sim.input.throttle = 1.0;
    for _ in 0..3000 { sim.step_10ms(); assert!(sim.state.v >= 0.0 && sim.state.v <= 3.0); }
    sim.input.throttle = 0.0;
    sim.input.brake = 1.0;
    for _ in 0..1000 { sim.step_10ms(); assert!(sim.state.v >= 0.0); }
}
