//! SDV core: Model layer + fixed-step simulation, usable natively and from
//! the browser (WebAssembly, plain C ABI - no wasm-bindgen required).

pub mod model;
pub mod safety;
pub mod sim;

use sim::{Row, Sim};
use std::sync::Mutex;

static SIM: Mutex<Option<Sim>> = Mutex::new(None);
static OUT: Mutex<[f32; 16]> = Mutex::new([0.0; 16]);

fn with_sim<R>(f: impl FnOnce(&mut Sim) -> R) -> R {
    let mut g = SIM.lock().unwrap();
    f(g.get_or_insert_with(Sim::new))
}

/// Layout of the buffer returned by `sdv_out_ptr`:
/// [t, x, y, yaw, v, yaw_rate, wheel_omega, throttle, brake, steer,
///  drive_accel_cmd, brake_decel_cmd, steer_angle_cmd, estop, system_state]
pub const OUT_LEN: usize = 15;

fn publish(row: &Row) {
    let mut o = OUT.lock().unwrap();
    *o = [
        row.state.t, row.state.x, row.state.y, row.state.yaw, row.state.v,
        row.state.yaw_rate, row.state.wheel_omega,
        row.input.throttle, row.input.brake, row.input.steer,
        row.cmd.drive_accel_cmd, row.cmd.brake_decel_cmd, row.cmd.steer_angle_cmd,
        row.safety.estop as u8 as f32, row.safety.system_state as u8 as f32, 0.0,
    ];
}

#[no_mangle]
pub extern "C" fn sdv_reset() {
    with_sim(|s| s.reset());
}

/// 1: follow the built-in demo scenario, 0: external (GUI) input.
#[no_mangle]
pub extern "C" fn sdv_use_scenario(on: i32) {
    with_sim(|s| s.use_scenario = on != 0);
}

#[no_mangle]
pub extern "C" fn sdv_set_input(throttle: f32, brake: f32, steer: f32) {
    with_sim(|s| {
        s.input.throttle = throttle.clamp(0.0, 1.0);
        s.input.brake = brake.clamp(0.0, 1.0);
        s.input.steer = steer.clamp(-1.0, 1.0);
    });
}

#[no_mangle]
pub extern "C" fn sdv_set_estop(on: i32) {
    with_sim(|s| s.safety.estop = on != 0);
}

/// Advance `n` 10 ms ticks; the last row is readable via `sdv_out_ptr`.
#[no_mangle]
pub extern "C" fn sdv_step(n: i32) {
    let last = with_sim(|s| {
        let mut last = None;
        for _ in 0..n.max(0) {
            last = Some(s.step_10ms());
        }
        last
    });
    if let Some(row) = last {
        publish(&row);
    }
}

#[no_mangle]
pub extern "C" fn sdv_out_ptr() -> *const f32 {
    // The buffer lives in a static, so the pointer stays valid.
    let g = OUT.lock().unwrap();
    g.as_ptr()
}

#[no_mangle]
pub extern "C" fn sdv_out_len() -> i32 {
    OUT_LEN as i32
}
