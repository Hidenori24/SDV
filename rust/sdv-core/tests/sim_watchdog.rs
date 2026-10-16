//! The watchdog wired into `Sim`: SWC stall injection must reproduce the C++ reference logs
//! (`sdv_sim --stall <swc>:<t>`) and behave like the C++ Safety SWC.

use sdv_core::sim::{Component, Row, Sim, SystemState};

/// Mirrors C++ `main.cpp`: a SWC is stalled once the *current* simulated time reaches `from_s`.
fn run_with_stalls(stalls: &[(Component, f32)], ticks: usize) -> Vec<Row> {
    let mut sim = Sim::new();
    sim.use_scenario = true;
    let mut rows = Vec::new();
    for _ in 0..ticks {
        for &(c, from) in stalls {
            if sim.state.t >= from {
                sim.set_stall(c, true);
            }
        }
        rows.push(sim.step_10ms());
    }
    rows
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

#[test]
fn one_stalled_swc_matches_cpp() {
    let rows = run_with_stalls(&[(Component::Brake, 2.0)], 1000);
    check_against(include_str!("golden/cpp_stall_brake2.csv"), &rows);
}

#[test]
fn two_stalled_swcs_escalate_to_estop_like_cpp() {
    let rows = run_with_stalls(&[(Component::Steering, 3.0), (Component::Brake, 3.5)], 1000);
    check_against(include_str!("golden/cpp_stall_steering3_brake3.5.csv"), &rows);
}

#[test]
fn stalled_vehicledynamics_freezes_time_like_cpp() {
    let rows = run_with_stalls(&[(Component::Engine, 1.0), (Component::VehicleDynamics, 2.0)], 1000);
    check_against(include_str!("golden/cpp_stall_engine1_vd2.csv"), &rows);
}

#[test]
fn healthy_sim_never_leaves_normal() {
    let mut sim = Sim::new();
    sim.input.throttle = 0.5;
    for _ in 0..6000 {
        sim.step_10ms();
        assert_eq!(sim.safety.system_state, SystemState::Normal);
        assert!(!sim.safety.estop);
    }
}

#[test]
fn degraded_recovers_when_swc_resumes() {
    let mut sim = Sim::new();
    sim.set_stall(Component::Steering, true);
    for _ in 0..150 { sim.step_10ms(); }
    assert_eq!(sim.safety.system_state, SystemState::Degraded);
    sim.set_stall(Component::Steering, false);
    for _ in 0..30 { sim.step_10ms(); }
    assert_eq!(sim.safety.system_state, SystemState::Normal);
}

#[test]
fn watchdog_estop_latches_until_reset() {
    let mut sim = Sim::new();
    sim.input.throttle = 1.0;
    sim.set_stall(Component::Engine, true);
    sim.set_stall(Component::Brake, true);
    for _ in 0..200 { sim.step_10ms(); }
    assert!(sim.safety.estop);
    assert_eq!(sim.safety.system_state, SystemState::EStop);

    sim.set_stall(Component::Engine, false);
    sim.set_stall(Component::Brake, false);
    sim.set_estop_request(false);
    for _ in 0..100 { sim.step_10ms(); }
    assert_eq!(sim.safety.system_state, SystemState::EStop, "latched");

    sim.reset();
    for _ in 0..30 { sim.step_10ms(); }
    assert_eq!(sim.safety.system_state, SystemState::Normal);
    assert!(!sim.safety.estop);
}

#[test]
fn external_estop_request_still_works_and_releases() {
    let mut sim = Sim::new();
    sim.input.throttle = 1.0;
    for _ in 0..300 { sim.step_10ms(); }
    sim.set_estop_request(true);
    for _ in 0..200 { sim.step_10ms(); }
    assert_eq!(sim.state.v, 0.0);
    assert_eq!(sim.safety.system_state, SystemState::EStop);
    sim.set_estop_request(false);
    for _ in 0..30 { sim.step_10ms(); }
    assert_eq!(sim.safety.system_state, SystemState::Normal);
}

#[test]
fn swc_status_reports_silence_per_component() {
    let mut sim = Sim::new();
    sim.set_stall(Component::Brake, true);
    for _ in 0..50 { sim.step_10ms(); }
    let st = sim.swc_status();
    assert!(st[Component::Engine as usize].alive && st[Component::Engine as usize].silent_ms == 0);
    assert!(st[Component::Brake as usize].alive && st[Component::Brake as usize].silent_ms > 0);
    for _ in 0..150 { sim.step_10ms(); }
    assert!(!sim.swc_status()[Component::Brake as usize].alive);
}
