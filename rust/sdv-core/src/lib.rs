//! SDV core: Model layer + fixed-step simulation, usable natively and from
//! the browser (WebAssembly, plain C ABI - no wasm-bindgen required).

pub mod bus;
pub mod config;
pub mod failsafe;
pub mod model;
pub mod nvm;
pub mod rte;
pub mod safety;
pub mod sim;
pub mod swc;

use bus::Node;
use sim::{Component, Row, Sim};
use std::sync::Mutex;

static SIM: Mutex<Option<Sim>> = Mutex::new(None);
static OUT: Mutex<[f32; 32]> = Mutex::new([0.0; 32]);

fn with_sim<R>(f: impl FnOnce(&mut Sim) -> R) -> R {
    let mut g = SIM.lock().unwrap();
    f(g.get_or_insert_with(Sim::new))
}

/// Layout of the buffer returned by `sdv_out_ptr`:
/// [t, x, y, yaw, v, yaw_rate, wheel_omega, throttle, brake, steer,
///  drive_accel_cmd, brake_decel_cmd, steer_angle_cmd, estop, system_state,
///  silent_ms[engine, brake, steering, vehicledynamics],
///  failsafe[engine ECU, brake ECU, steering ECU]]  (bit0: DriverInput timeout, bit1: Safety timeout)
pub const OUT_LEN: usize = 22;

fn publish(row: &Row, status: &[safety::SwcStatus; 4], failsafe: &[failsafe::FailsafeStatus; 3]) {
    let mut o = OUT.lock().unwrap();
    o[..15].copy_from_slice(&[
        row.state.t, row.state.x, row.state.y, row.state.yaw, row.state.v,
        row.state.yaw_rate, row.state.wheel_omega,
        row.input.throttle, row.input.brake, row.input.steer,
        row.cmd.drive_accel_cmd, row.cmd.brake_decel_cmd, row.cmd.steer_angle_cmd,
        row.safety.estop as u8 as f32, row.safety.system_state as u8 as f32,
    ]);
    for (i, st) in status.iter().enumerate() {
        o[15 + i] = st.silent_ms as f32;
    }
    for (i, f) in failsafe.iter().enumerate() {
        o[19 + i] = (f.driver_timeout as u8 | (f.safety_timeout as u8) << 1) as f32;
    }
}

#[no_mangle]
pub extern "C" fn sdv_reset() {
    with_sim(|s| s.reset());
}

/// 1: follow the built-in demo scenario, 0: external (GUI) input.
#[no_mangle]
pub extern "C" fn sdv_use_scenario(on: i32) {
    with_sim(|s| s.set_use_scenario(on != 0));
}

#[no_mangle]
pub extern "C" fn sdv_set_input(throttle: f32, brake: f32, steer: f32) {
    with_sim(|s| {
        s.input_mut().throttle = throttle.clamp(0.0, 1.0);
        s.input_mut().brake = brake.clamp(0.0, 1.0);
        s.input_mut().steer = steer.clamp(-1.0, 1.0);
    });
}

#[no_mangle]
pub extern "C" fn sdv_set_estop(on: i32) {
    with_sim(|s| s.set_estop_request(on != 0));
}

/// Fault injection: stall (1) / resume (0) a SWC. `component`: 0 engine, 1 brake, 2 steering, 3 vehicledynamics.
#[no_mangle]
pub extern "C" fn sdv_set_stall(component: i32, on: i32) {
    let c = match component {
        0 => Component::Engine,
        1 => Component::Brake,
        2 => Component::Steering,
        3 => Component::VehicleDynamics,
        _ => return,
    };
    with_sim(|s| s.set_stall(c, on != 0));
}

/// Vehicle-bus faults: delay in ticks (10 ms), frame loss in per mille, `down` = 1 loses every frame.
/// The direct ECU -> plant links are not affected.
#[no_mangle]
pub extern "C" fn sdv_set_bus(delay_ticks: i32, loss_permille: i32, down: i32) {
    with_sim(|s| {
        s.set_bus_delay_ticks(delay_ticks.max(0) as u64);
        s.set_bus_loss_permille(loss_permille.max(0) as u32);
        s.set_bus_down(down != 0);
    });
}

/// Stall (1) / resume (0) the VCU (DriverInput + Safety).
#[no_mangle]
pub extern "C" fn sdv_set_vcu_stall(on: i32) {
    with_sim(|s| s.set_vcu_stall(on != 0));
}

/// Advance `n` 10 ms ticks; the last row is readable via `sdv_out_ptr`.
#[no_mangle]
pub extern "C" fn sdv_step(n: i32) {
    let last = with_sim(|s| {
        let mut last = None;
        for _ in 0..n.max(0) {
            last = Some(s.step_10ms());
        }
        last.map(|row| {
            let fs = [Node::Engine, Node::Brake, Node::Steering].map(|n| s.ecu_failsafe(n));
            (row, s.swc_status(), fs)
        })
    });
    if let Some((row, status, fs)) = last {
        publish(&row, &status, &fs);
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
