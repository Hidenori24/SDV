//! Bus faults (delay / loss / down), VCU stall and the receiving ECUs' fail-safe (ADR-0002).
//! The ECU -> Plant actuator link is direct wiring: bus faults do not affect it.

use sdv_core::bus::Node;
use sdv_core::sim::{Component, Sim, SystemState};

fn cruise(sim: &mut Sim, ticks: usize) {
    sim.input_mut().throttle = 1.0;
    for _ in 0..ticks {
        sim.step_10ms();
    }
}

fn first_tick_where(sim: &mut Sim, max: usize, pred: impl Fn(&Sim) -> bool) -> Option<usize> {
    for t in 0..max {
        sim.step_10ms();
        if pred(sim) {
            return Some(t);
        }
    }
    None
}

#[test]
fn bus_delay_postpones_the_driver_input() {
    let mut a = Sim::new();
    a.input_mut().throttle = 1.0;
    let base = first_tick_where(&mut a, 100, |s| s.cmd().drive_accel_cmd > 0.0).unwrap();

    let mut b = Sim::new();
    b.set_bus_delay_ticks(5);
    b.input_mut().throttle = 1.0;
    let delayed = first_tick_where(&mut b, 100, |s| s.cmd().drive_accel_cmd > 0.0).unwrap();
    assert_eq!(delayed, base + 5);
}

#[test]
fn actuator_link_to_the_plant_is_not_delayed() {
    // Engine -> Plant is direct: even with a slow bus, the command computed in a tick reaches the plant in that tick.
    let mut s = Sim::new();
    s.set_bus_delay_ticks(5);
    s.input_mut().throttle = 1.0;
    for _ in 0..20 {
        s.step_10ms();
    }
    assert_eq!(s.rte(Node::Engine).cmd.drive_accel_cmd, s.rte(Node::Plant).cmd.drive_accel_cmd);
}

#[test]
fn bus_loss_is_deterministic_and_reproducible() {
    let run = || {
        let mut s = Sim::new();
        s.set_bus_loss_permille(300);
        s.input_mut().throttle = 0.8;
        let mut rows = Vec::new();
        for _ in 0..300 {
            rows.push(s.step_10ms().to_csv());
        }
        rows
    };
    assert_eq!(run(), run());
}

#[test]
fn a_few_lost_frames_do_not_trigger_failsafe() {
    // 10 % loss: with a frame every 10 ms, 11 consecutive losses are practically impossible here.
    let mut s = Sim::new();
    s.set_bus_loss_permille(100);
    s.set_use_scenario(true);
    for _ in 0..1000 {
        s.step_10ms();
        assert_eq!(s.safety().system_state, SystemState::Normal);
    }
    assert!(!s.ecu_failsafe(Node::Engine).safety_timeout);
}

#[test]
fn bus_down_makes_every_ecu_fail_safe_and_the_car_stops() {
    let mut s = Sim::new();
    cruise(&mut s, 300);
    assert!(s.state().v > 1.0);
    s.set_bus_down(true);
    // Safety frames stop: after > 100 ms each ECU enters EStop behaviour by itself
    for _ in 0..15 {
        s.step_10ms();
    }
    for node in [Node::Engine, Node::Brake, Node::Steering] {
        assert!(s.ecu_failsafe(node).safety_timeout, "{node:?}");
    }
    assert_eq!(s.rte(Node::Engine).cmd.drive_accel_cmd, 0.0);
    assert_eq!(s.rte(Node::Brake).cmd.brake_decel_cmd, 4.0);
    for _ in 0..400 {
        s.step_10ms();
    }
    assert_eq!(s.state().v, 0.0);
}

#[test]
fn bus_down_is_detected_by_the_vcu_watchdog_after_the_timeout() {
    let mut s = Sim::new();
    cruise(&mut s, 100);
    s.set_bus_down(true);
    for _ in 0..90 {
        s.step_10ms();
    }
    assert_eq!(s.safety().system_state, SystemState::Normal, "within 1 s of silence");
    for _ in 0..60 {
        s.step_10ms();
    }
    // all four alive counters stopped -> EStop (latched)
    assert_eq!(s.safety().system_state, SystemState::EStop);
}

#[test]
fn short_bus_outage_recovers_without_a_latch() {
    let mut s = Sim::new();
    cruise(&mut s, 200);
    s.set_bus_down(true);
    for _ in 0..30 {
        s.step_10ms(); // 300 ms: ECUs fail safe, VCU watchdog still Normal
    }
    assert!(s.ecu_failsafe(Node::Engine).safety_timeout);
    s.set_bus_down(false);
    for _ in 0..30 {
        s.step_10ms();
    }
    assert!(!s.ecu_failsafe(Node::Engine).safety_timeout);
    assert_eq!(s.safety().system_state, SystemState::Normal);
    assert!(s.rte(Node::Engine).cmd.drive_accel_cmd > 0.0, "throttle is back");
}

#[test]
fn vcu_stall_stops_the_car_through_the_ecus_own_failsafe() {
    let mut s = Sim::new();
    cruise(&mut s, 300);
    assert!(s.state().v > 1.0);
    s.set_vcu_stall(true);
    for _ in 0..500 {
        s.step_10ms();
    }
    assert!(s.ecu_failsafe(Node::Brake).safety_timeout);
    assert_eq!(s.state().v, 0.0);
    // the stalled VCU itself shows nothing new
    assert_eq!(s.safety().system_state, SystemState::Normal);
}

#[test]
fn driver_input_loss_alone_zeroes_the_input_but_keeps_running() {
    // Stall only the DriverInput path by dropping everything for > 200 ms but < 1 s watchdog.
    let mut s = Sim::new();
    cruise(&mut s, 200);
    s.set_bus_down(true);
    for _ in 0..25 {
        s.step_10ms();
    }
    assert!(s.ecu_failsafe(Node::Engine).driver_timeout);
    assert_eq!(s.rte(Node::Engine).input.throttle, 0.0);
}

#[test]
fn ecu_stall_and_bus_faults_compose() {
    let mut s = Sim::new();
    s.set_stall(Component::Engine, true);
    s.set_bus_delay_ticks(2);
    for _ in 0..200 {
        s.step_10ms();
    }
    assert_eq!(s.safety().system_state, SystemState::Degraded);
}

#[test]
fn reset_clears_bus_faults() {
    let mut s = Sim::new();
    s.set_bus_down(true);
    s.set_vcu_stall(true);
    s.reset();
    cruise(&mut s, 50);
    assert!(s.state().v > 0.0);
    assert!(!s.ecu_failsafe(Node::Engine).safety_timeout);
}
