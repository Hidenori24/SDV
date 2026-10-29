//! SDV core: Model layer + fixed-step simulation, usable natively and from
//! the browser (WebAssembly, plain C ABI - no wasm-bindgen required).

pub mod bus;
pub mod config;
pub mod failsafe;
#[cfg(feature = "fota-simulator")]
pub mod fota;
pub mod model;
pub mod nvm;
pub mod platform;
pub mod rte;
pub mod runtime;
pub mod scenario;
pub mod safety;
pub mod sim;
pub mod swc;

use bus::Node;
use nvm::{MemStorage, Nvm, Slot, SlotStatus, Source};
use sim::{Component, Row, Sim};
use std::sync::Mutex;

static SIM: Mutex<Option<Sim>> = Mutex::new(None);
static OUT: Mutex<[f32; 40]> = Mutex::new([0.0; 40]);

static NVM: Mutex<Option<Nvm<MemStorage>>> = Mutex::new(None);
/// Scratch buffer the host reads after calls that return text / bytes (`sdv_buf_ptr` / `sdv_buf_len`).
static BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn with_nvm<R>(f: impl FnOnce(&mut Nvm<MemStorage>) -> R) -> R {
    let mut g = NVM.lock().unwrap();
    f(g.get_or_insert_with(|| Nvm::new(MemStorage::default())))
}

fn set_buf(bytes: Vec<u8>) {
    *BUF.lock().unwrap() = bytes;
}

fn slot_of(i: i32) -> Slot {
    if i == 0 { Slot::Active } else { Slot::Backup }
}

fn with_sim<R>(f: impl FnOnce(&mut Sim) -> R) -> R {
    let mut g = SIM.lock().unwrap();
    f(g.get_or_insert_with(Sim::new))
}

/// Layout of the buffer returned by `sdv_out_ptr`:
/// [t, x, y, yaw, v, yaw_rate, wheel_omega, throttle, brake, steer,
///  drive_accel_cmd, brake_decel_cmd, steer_angle_cmd, estop, system_state,
///  silent_ms[engine, brake, steering, vehicledynamics],
///  failsafe[engine ECU, brake ECU, steering ECU],  (bit0: DriverInput timeout, bit1: Safety timeout)
///  injected faults: stalled[engine, brake, steering, plant, vcu], bus_down]  (1.0 = active)
pub const OUT_LEN: usize = 28;

fn publish(row: &Row, status: &[safety::SwcStatus; 4], failsafe: &[failsafe::FailsafeStatus; 3], faults: &[f32; 6]) {
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
    o[22..28].copy_from_slice(faults);
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
            let f = |b: bool| b as u8 as f32;
            let faults = [
                f(s.is_stalled(Component::Engine)),
                f(s.is_stalled(Component::Brake)),
                f(s.is_stalled(Component::Steering)),
                f(s.is_stalled(Component::VehicleDynamics)),
                f(s.vcu_stalled()),
                f(s.bus_is_down()),
            ];
            (row, s.swc_status(), fs, faults)
        })
    });
    if let Some((row, status, fs, faults)) = last {
        publish(&row, &status, &fs, &faults);
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

// ---------------------------------------------------------------------------
// Config / NVM (ADR-0003). Text and bytes go through host-allocated memory and
// the scratch buffer.
// ---------------------------------------------------------------------------

/// Allocate `n` bytes of WASM memory for the host to write into. Release with `sdv_free`.
#[no_mangle]
pub extern "C" fn sdv_alloc(n: i32) -> *mut u8 {
    let mut v = vec![0u8; n.max(0) as usize];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety contract
/// `ptr` / `n` must be a pair previously returned by / passed to `sdv_alloc`.
#[no_mangle]
pub extern "C" fn sdv_free(ptr: *mut u8, n: i32) {
    if !ptr.is_null() && n >= 0 {
        // SAFETY: the pair comes from `sdv_alloc`, which leaked a Vec of exactly this length/capacity.
        unsafe { drop(Vec::from_raw_parts(ptr, n as usize, n as usize)) }
    }
}

fn host_bytes<'a>(ptr: *const u8, n: i32) -> &'a [u8] {
    if ptr.is_null() || n <= 0 {
        return &[];
    }
    // SAFETY: the host passes a region it allocated with `sdv_alloc` and filled.
    unsafe { std::slice::from_raw_parts(ptr, n as usize) }
}

#[no_mangle]
pub extern "C" fn sdv_buf_ptr() -> *const u8 {
    BUF.lock().unwrap().as_ptr()
}

#[no_mangle]
pub extern "C" fn sdv_buf_len() -> i32 {
    BUF.lock().unwrap().len() as i32
}

/// Boot: load the config from the NVM (Active, else Backup, else defaults) and restart the sim with it.
/// Returns the source: 0 active, 1 backup, 2 defaults.
#[no_mangle]
pub extern "C" fn sdv_boot() -> i32 {
    let loaded = with_nvm(|n| n.load());
    with_sim(|s| s.set_config(loaded.config));
    match loaded.source {
        Source::Active => 0,
        Source::Backup => 1,
        Source::Defaults => 2,
    }
}

/// Put the JSON of the running config into the buffer.
#[no_mangle]
pub extern "C" fn sdv_config_json() {
    let json = with_sim(|s| config::to_json(s.config()));
    set_buf(json.into_bytes());
}

/// Put the derived values of the running config (minimum turn radius, lateral acceleration limit,
/// critical speed, and the full-lock turn curve over the speed range) into the buffer as JSON.
#[no_mangle]
pub extern "C" fn sdv_derived_json() {
    let json = with_sim(|s| s.config().derived_json());
    set_buf(json.into_bytes());
}

/// Largest steering angle [rad] that takes effect at speed `v` [m/s] on the running config (tyre grip limit).
/// The host uses it to shape driver input (e.g. keyboard steering) without duplicating the formula.
#[no_mangle]
pub extern "C" fn sdv_max_steer_at_speed(v: f32) -> f32 {
    with_sim(|s| s.config().max_steer_at_speed(v))
}

/// Same as `sdv_derived_json`, but for a config JSON being edited (not stored, not applied): lets the host show
/// the derived values live. 0 ok (report in the buffer), 1 invalid (the problems, one `path: reason` per line,
/// are in the buffer), 2 not UTF-8.
#[no_mangle]
pub extern "C" fn sdv_derived_json_for(ptr: *const u8, n: i32) -> i32 {
    let Ok(json) = std::str::from_utf8(host_bytes(ptr, n)) else {
        return 2;
    };
    match config::parse(json) {
        Ok(c) => {
            set_buf(c.derived_json().into_bytes());
            0
        }
        Err(errors) => {
            set_buf(errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n").into_bytes());
            1
        }
    }
}

/// Put the JSON of the built-in default config into the buffer.
#[no_mangle]
pub extern "C" fn sdv_default_config_json() {
    set_buf(config::to_json(&config::Config::default()).into_bytes());
}

/// Validate and store a config JSON. 0 ok, 1 invalid (the problems, one `path: reason` per line, are in the buffer),
/// 2 not UTF-8. Nothing is stored unless 0. It takes effect on the next `sdv_boot`.
#[no_mangle]
pub extern "C" fn sdv_nvm_commit(ptr: *const u8, n: i32) -> i32 {
    let Ok(json) = std::str::from_utf8(host_bytes(ptr, n)) else {
        return 2;
    };
    match with_nvm(|nvm| nvm.commit(json)) {
        Ok(_) => 0,
        Err(nvm::NvmError::Invalid(errors)) => {
            let text = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
            set_buf(text.into_bytes());
            1
        }
        Err(nvm::NvmError::NoBackup) => 1,
    }
}

/// `Active <- Backup`. 0 ok, 1 no valid backup. Takes effect on the next `sdv_boot`.
#[no_mangle]
pub extern "C" fn sdv_nvm_rollback() -> i32 {
    match with_nvm(|nvm| nvm.rollback()) {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

/// Slot status: -1 empty, -2 corrupt, otherwise the sequence number. `slot`: 0 active, 1 backup.
#[no_mangle]
pub extern "C" fn sdv_nvm_slot_status(slot: i32) -> i32 {
    match with_nvm(|nvm| nvm.load().slots[slot_of(slot) as usize]) {
        SlotStatus::Empty => -1,
        SlotStatus::Corrupt => -2,
        SlotStatus::Valid { seq } => seq as i32,
    }
}

/// Copy the raw bytes of a slot into the buffer (to persist them). 1 if the slot has data, 0 if empty.
#[no_mangle]
pub extern "C" fn sdv_nvm_read(slot: i32) -> i32 {
    match with_nvm(|nvm| nvm.storage().read(slot_of(slot))) {
        Some(bytes) => {
            set_buf(bytes);
            1
        }
        None => {
            set_buf(Vec::new());
            0
        }
    }
}

/// Restore the raw bytes of a slot (previously saved with `sdv_nvm_read`).
#[no_mangle]
pub extern "C" fn sdv_nvm_write(slot: i32, ptr: *const u8, n: i32) {
    let bytes = host_bytes(ptr, n).to_vec();
    with_nvm(|nvm| nvm.storage_mut().write(slot_of(slot), bytes));
}

#[no_mangle]
pub extern "C" fn sdv_nvm_erase(slot: i32) {
    with_nvm(|nvm| nvm.storage_mut().erase(slot_of(slot)));
}

/// Fault injection: flip a byte in the middle of a slot.
#[no_mangle]
pub extern "C" fn sdv_nvm_corrupt(slot: i32) {
    with_nvm(|nvm| {
        let slot = slot_of(slot);
        if let Some(mut bytes) = nvm.storage().read(slot) {
            let mid = bytes.len() / 2;
            if let Some(b) = bytes.get_mut(mid) {
                *b ^= 0xFF;
            }
            nvm.storage_mut().write(slot, bytes);
        }
    });
}

/// Validate and install a scenario JSON (inputs + timed events). 0 ok, 1 invalid (the problems, one
/// `path: reason` per line, are in the buffer; the running scenario is unchanged), 2 not UTF-8.
/// Scenario mode is switched with `sdv_use_scenario`; `sdv_reset` replays the scenario from the start.
#[no_mangle]
pub extern "C" fn sdv_set_scenario(ptr: *const u8, n: i32) -> i32 {
    let Ok(json) = std::str::from_utf8(host_bytes(ptr, n)) else {
        return 2;
    };
    match scenario::parse(json) {
        Ok(sc) => {
            with_sim(|s| s.set_scenario(sc));
            0
        }
        Err(errors) => {
            let text = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
            set_buf(text.into_bytes());
            1
        }
    }
}

/// Put the JSON of the running scenario into the buffer.
#[no_mangle]
pub extern "C" fn sdv_scenario_json() {
    let json = with_sim(|s| scenario::to_json(s.scenario()));
    set_buf(json.into_bytes());
}

/// Put the JSON of the built-in demo scenario into the buffer.
#[no_mangle]
pub extern "C" fn sdv_demo_scenario_json() {
    set_buf(scenario::to_json(&scenario::Scenario::demo()).into_bytes());
}
