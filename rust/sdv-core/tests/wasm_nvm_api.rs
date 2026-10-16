//! C ABI for Config / NVM as used by the browser. Global state => one sequential test per binary.

use sdv_core::*;

fn buf_bytes() -> Vec<u8> {
    let n = sdv_buf_len() as usize;
    unsafe { std::slice::from_raw_parts(sdv_buf_ptr(), n) }.to_vec()
}

fn buf() -> String {
    String::from_utf8_lossy(&buf_bytes()).to_string()
}

fn put(bytes: &[u8]) -> (*mut u8, i32) {
    let p = sdv_alloc(bytes.len() as i32);
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    (p, bytes.len() as i32)
}

fn commit(json: &str) -> i32 {
    let (p, n) = put(json.as_bytes());
    let r = sdv_nvm_commit(p, n);
    sdv_free(p, n);
    r
}

fn speed_after_half_second() -> f32 {
    sdv_reset();
    sdv_use_scenario(0);
    sdv_set_input(1.0, 0.0, 0.0);
    sdv_step(50);
    let out = unsafe { std::slice::from_raw_parts(sdv_out_ptr(), sdv_out_len() as usize) };
    out[4]
}

#[test]
fn config_and_nvm_through_the_c_api() {
    // fresh device: defaults
    assert_eq!(sdv_boot(), 2, "source: 0 active, 1 backup, 2 defaults");
    assert_eq!(sdv_nvm_slot_status(0), -1);
    sdv_config_json();
    assert!(buf().contains("\"max_accel_mps2\": 2.0"));
    let slow = speed_after_half_second();

    // derived values of the running config
    sdv_derived_json();
    let d = buf();
    assert!(d.contains("min_turn_radius_m") && d.contains("max_lateral_accel_mps2") && d.contains("turn_curve"), "{d}");

    // commit a valid config and reboot with it
    assert_eq!(commit(r#"{"engine":{"max_accel_mps2":4.0}}"#), 0);
    assert_eq!(sdv_nvm_slot_status(0), 1, "seq 1");
    assert_eq!(sdv_boot(), 0);
    let fast = speed_after_half_second();
    assert!(fast > slow * 1.8, "slow {slow} fast {fast}");

    // an invalid config is rejected, the errors are readable, nothing changes
    assert_eq!(commit(r#"{"engine":{"max_accel_mps2":99},"safety":{"heartbeat_timeout_ms":1}}"#), 1);
    let e = buf();
    assert!(e.contains("vehicle_spec.max_accel_mps2") && e.contains("tuning.safety.heartbeat_timeout_ms"), "{e}");
    assert_eq!(sdv_nvm_slot_status(0), 1);

    // second commit, then rollback
    assert_eq!(commit(r#"{"engine":{"max_accel_mps2":1.0}}"#), 0);
    assert_eq!(sdv_nvm_slot_status(1), 1, "previous config is in the backup");
    assert_eq!(sdv_nvm_rollback(), 0);
    assert_eq!(sdv_boot(), 0);
    assert!((speed_after_half_second() - fast).abs() < 1e-4, "rolled back to the 4.0 config");
    assert_eq!(sdv_nvm_rollback(), 1, "no backup left");

    // persistence hooks: read a slot, wipe the device, restore the bytes
    assert_eq!(sdv_nvm_read(0), 1);
    let saved = buf_bytes();
    assert_eq!(sdv_nvm_read(1), 0, "backup is empty");

    // corruption injection: active broken -> falls back to backup (or defaults)
    assert_eq!(commit(r#"{"engine":{"max_accel_mps2":3.0}}"#), 0);
    sdv_nvm_corrupt(0);
    assert_eq!(sdv_nvm_slot_status(0), -2);
    assert_eq!(sdv_boot(), 1, "fell back to the backup");

    // wipe + restore
    sdv_nvm_erase(0);
    sdv_nvm_erase(1);
    assert_eq!(sdv_boot(), 2);
    let (p, n) = put(&saved);
    sdv_nvm_write(0, p, n);
    sdv_free(p, n);
    assert_eq!(sdv_nvm_slot_status(0) >= 0, true);
    assert_eq!(sdv_boot(), 0);
}
