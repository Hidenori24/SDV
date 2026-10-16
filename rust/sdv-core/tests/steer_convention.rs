//! Sign convention of `steer` (shared with C++): positive steer = LEFT turn
//! (counter-clockwise seen from above, yaw increases, +y when heading +x).
//! The GUI maps "right" input to a negative steer; see `web/app.js`.

use sdv_core::sim::Sim;

fn drive(steer: f32) -> Sim {
    let mut sim = Sim::new();
    sim.input_mut().throttle = 0.6;
    sim.input_mut().steer = steer;
    for _ in 0..300 {
        sim.step_10ms();
    }
    sim
}

#[test]
fn positive_steer_turns_left() {
    let s = drive(0.5);
    assert!(s.state().yaw > 0.0 && s.state().yaw_rate > 0.0, "yaw={} rate={}", s.state().yaw, s.state().yaw_rate);
    assert!(s.state().y > 0.0, "left turn from heading +x must move toward +y, y={}", s.state().y);
}

#[test]
fn negative_steer_turns_right() {
    let s = drive(-0.5);
    assert!(s.state().yaw < 0.0 && s.state().yaw_rate < 0.0);
    assert!(s.state().y < 0.0);
}

#[test]
fn left_and_right_are_mirror_images() {
    let l = drive(0.5);
    let r = drive(-0.5);
    assert!((l.state().x - r.state().x).abs() < 1e-3);
    assert!((l.state().y + r.state().y).abs() < 1e-3);
    assert!((l.state().yaw + r.state().yaw).abs() < 1e-4);
}
