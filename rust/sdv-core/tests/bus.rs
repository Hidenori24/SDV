use sdv_core::bus::{Bus, Frame, Node};
use sdv_core::rte::{DriverInput, Rte};

#[test]
fn zero_delay_frames_are_delivered_in_the_same_tick() {
    let mut bus = Bus::new(0);
    bus.send(5, Frame::CmdDrive(1.5));
    let got = bus.deliver(5);
    assert_eq!(got, vec![(Node::Plant, Frame::CmdDrive(1.5))]);
    assert_eq!(bus.in_flight(), 0);
}

#[test]
fn delayed_frames_arrive_after_the_delay_in_send_order() {
    let mut bus = Bus::new(3);
    bus.send(0, Frame::CmdDrive(1.0));
    bus.send(1, Frame::CmdDrive(2.0));
    assert!(bus.deliver(2).is_empty());
    assert_eq!(bus.deliver(3), vec![(Node::Plant, Frame::CmdDrive(1.0))]);
    assert_eq!(bus.deliver(4), vec![(Node::Plant, Frame::CmdDrive(2.0))]);
}

#[test]
fn bus_matrix_matches_the_adr() {
    let d = Frame::DriverInput(DriverInput::default());
    assert_eq!(d.receivers(), &[Node::Engine, Node::Brake, Node::Steering]);
    assert!(Frame::Safety(Default::default()).receivers().contains(&Node::Plant));
    assert!(!Frame::Safety(Default::default()).receivers().contains(&Node::Vcu));
    assert_eq!(Frame::Alive { index: 0, counter: 1 }.receivers(), &[Node::Vcu]);
    assert_eq!(Frame::CmdSteer(0.1).receivers(), &[Node::Plant]);
}

#[test]
fn a_frame_to_several_receivers_is_delivered_to_each() {
    let mut bus = Bus::new(0);
    bus.send(0, Frame::DriverInput(DriverInput { throttle: 0.5, brake: 0.0, steer: 0.0 }));
    let got = bus.deliver(0);
    assert_eq!(got.len(), 3);
}

#[test]
fn apply_writes_only_the_frames_field() {
    let mut rte = Rte::default();
    Frame::CmdBrake(2.0).apply(&mut rte);
    assert_eq!(rte.cmd.brake_decel_cmd, 2.0);
    assert_eq!(rte.cmd.drive_accel_cmd, 0.0);
    Frame::Alive { index: 2, counter: 7 }.apply(&mut rte);
    assert_eq!(rte.alive_rx, [0, 0, 7, 0]);
}
