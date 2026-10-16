//! SWCs only touch the `Rte` they are given, so several RTE instances (= ECUs) can coexist.

use sdv_core::rte::{Rte, SystemState};
use sdv_core::swc::*;

#[test]
fn engine_and_brake_write_only_their_own_outputs() {
    let mut rte = Rte::default();
    rte.input.throttle = 0.5;
    rte.input.brake = 0.5;
    EngineSwc::default().main_10ms(&mut rte);
    assert_eq!(rte.cmd.drive_accel_cmd, 1.0);
    assert_eq!(rte.cmd.brake_decel_cmd, 0.0, "Brake has not run yet");
    BrakeSwc::default().main_10ms(&mut rte);
    assert_eq!(rte.cmd.brake_decel_cmd, 2.0);
    assert_eq!(rte.cmd.steer_angle_cmd, 0.0);
}

#[test]
fn two_rtes_are_independent() {
    let (mut a, mut b) = (Rte::default(), Rte::default());
    a.input.throttle = 1.0;
    b.input.throttle = 0.25;
    let engine = EngineSwc::default();
    engine.main_10ms(&mut a);
    engine.main_10ms(&mut b);
    assert_eq!(a.cmd.drive_accel_cmd, 2.0);
    assert_eq!(b.cmd.drive_accel_cmd, 0.5);
}

#[test]
fn estop_in_the_rte_overrides_swc_outputs() {
    let mut rte = Rte::default();
    rte.input.throttle = 1.0;
    rte.input.steer = 1.0;
    rte.safety.system_state = SystemState::EStop;
    EngineSwc::default().main_10ms(&mut rte);
    BrakeSwc::default().main_10ms(&mut rte);
    let mut steering = SteeringSwc::default();
    for _ in 0..100 { steering.main_10ms(&mut rte, 0.01); }
    assert_eq!(rte.cmd.drive_accel_cmd, 0.0);
    assert_eq!(rte.cmd.brake_decel_cmd, 4.0);
    assert_eq!(rte.cmd.steer_angle_cmd, 0.0);
}

#[test]
fn vehicle_dynamics_advances_time_and_speed() {
    let mut rte = Rte::default();
    rte.cmd.drive_accel_cmd = 2.0;
    let vd = VehicleDynamicsSwc::default();
    for _ in 0..100 { vd.step_10ms(&mut rte, 0.01); }
    assert!((rte.state.t - 1.0).abs() < 1e-4);
    assert!(rte.state.v > 1.0);
}

#[test]
fn scenario_input_follows_simulated_time() {
    let mut rte = Rte::default();
    rte.state.t = 3.0;
    DriverInputSwc::main_20ms_scenario(&mut rte, &sdv_core::scenario::Scenario::demo());
    assert_eq!((rte.input.throttle, rte.input.steer), (0.2, 0.2));
}
