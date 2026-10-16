//! ECU split (ADR-0002, plan A): VCU / Engine / Brake / Steering ECUs + Plant on a virtual bus.
//! With zero delay and zero loss the system must behave exactly like the single-process version.

use sdv_core::bus::Node;
use sdv_core::sim::{Component, Sim, SystemState};

#[test]
fn each_ecu_has_its_own_rte() {
    let mut sim = Sim::new();
    sim.input_mut().throttle = 0.5;
    sim.input_mut().brake = 0.25;
    sim.input_mut().steer = 0.1;
    for _ in 0..4 {
        sim.step_10ms();
    }
    // The driver input only exists on the VCU until the bus delivers it; once delivered each
    // ECU holds its own copy, and outputs live on the ECU that computed them.
    let engine = sim.rte(Node::Engine);
    let brake = sim.rte(Node::Brake);
    assert_eq!(engine.input.throttle, 0.5);
    assert_eq!(engine.cmd.drive_accel_cmd, 1.0);
    assert_eq!(engine.cmd.brake_decel_cmd, 0.0, "Engine ECU does not run Brake");
    assert_eq!(brake.cmd.brake_decel_cmd, 1.0);
    assert_eq!(brake.cmd.drive_accel_cmd, 0.0, "Brake ECU does not run Engine");
}

#[test]
fn plant_receives_actuator_commands_over_the_bus() {
    let mut sim = Sim::new();
    sim.input_mut().throttle = 1.0;
    for _ in 0..50 {
        sim.step_10ms();
    }
    let plant = sim.rte(Node::Plant);
    assert_eq!(plant.cmd.drive_accel_cmd, 2.0);
    assert!(plant.state.v > 0.5);
    // The VCU learns the vehicle state only through the VehicleState frame
    assert!((sim.rte(Node::Vcu).state.t - plant.state.t).abs() < 1e-6);
}

#[test]
fn stalled_ecu_stops_sending_frames_and_plant_keeps_stale_command() {
    let mut sim = Sim::new();
    sim.input_mut().throttle = 1.0;
    for _ in 0..50 {
        sim.step_10ms();
    }
    sim.set_stall(Component::Engine, true);
    sim.input_mut().throttle = 0.0;
    for _ in 0..50 {
        sim.step_10ms();
    }
    assert_eq!(sim.rte(Node::Engine).cmd.drive_accel_cmd, 2.0, "stalled ECU does not update");
    assert_eq!(sim.rte(Node::Plant).cmd.drive_accel_cmd, 2.0, "stale command stays on the plant");
}

#[test]
fn vcu_watchdog_sees_ecu_alive_counters_via_the_bus() {
    let mut sim = Sim::new();
    sim.set_stall(Component::Steering, true);
    sim.set_stall(Component::Brake, true);
    for _ in 0..200 {
        sim.step_10ms();
    }
    assert_eq!(sim.safety().system_state, SystemState::EStop);
    // every ECU received the Safety frame
    assert!(sim.rte(Node::Engine).safety.estop);
    assert!(sim.rte(Node::Plant).safety.estop);
}
