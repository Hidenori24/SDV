//! Trial application of a config through the C ABI (global state => one sequential test per binary).

use sdv_core::*;

const TRIAL_STATE: usize = 28;
const TRIAL_REMAINING_MS: usize = 29;
const TRIAL_REASON: usize = 30;
const CONFIG_SEQ: usize = 31;

fn buf() -> String {
    String::from_utf8_lossy(unsafe {
        std::slice::from_raw_parts(sdv_buf_ptr(), sdv_buf_len() as usize)
    })
    .to_string()
}

fn out() -> Vec<f32> {
    unsafe { std::slice::from_raw_parts(sdv_out_ptr(), sdv_out_len() as usize) }.to_vec()
}

fn with_text(s: &str, f: impl FnOnce(*const u8, i32) -> i32) -> i32 {
    let p = sdv_alloc(s.len() as i32);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len()) };
    let r = f(p, s.len() as i32);
    unsafe { sdv_free(p, s.len() as i32) };
    r
}

fn commit(json: &str) {
    assert_eq!(with_text(json, |p, n| sdv_nvm_commit(p, n)), 0, "{}", buf());
}

fn set_scenario(json: &str) {
    assert_eq!(
        with_text(json, |p, n| sdv_set_scenario(p, n)),
        0,
        "{}",
        buf()
    );
    sdv_use_scenario(1);
}

fn factory_reset() {
    sdv_nvm_erase(0);
    sdv_nvm_erase(1);
    sdv_boot();
    set_scenario(r#"{"steps":[{"until_s":60,"throttle":0.5}]}"#);
    sdv_reset();
}

fn config_json() -> String {
    sdv_config_json();
    buf()
}

fn step_seconds(s: f32) {
    sdv_step((s * 100.0).round() as i32);
}

/// A Watchdog so strict that a half-second bus outage looks like a dead ECU (the default 1000 ms ignores it).
const STRICT: &str = r#"{"tuning":{"safety":{"heartbeat_timeout_ms":200}}}"#;
const BUS_OUTAGE: &str = r#"{"steps":[{"until_s":60,"throttle":0.5}],
    "events":[{"at_s":1.0,"action":"bus_down"},{"at_s":1.5,"action":"bus_up"}]}"#;

#[test]
fn a_new_config_runs_on_probation_and_is_rolled_back_when_it_makes_the_system_unhealthy() {
    // --- the window must be sensible
    factory_reset();
    assert_eq!(sdv_trial_start(0), 1);
    assert_eq!(sdv_trial_start(-5), 1);
    assert_eq!(sdv_trial_start(60_001), 1);
    sdv_step(1);
    assert_eq!(out()[TRIAL_STATE], 0.0, "refused starts leave no trial");

    // --- a healthy update passes its trial and stays
    factory_reset();
    commit(r#"{"vehicle_spec":{"max_accel_mps2":3.0}}"#);
    sdv_boot();
    set_scenario(r#"{"steps":[{"until_s":60,"throttle":0.5}]}"#);
    sdv_reset();
    assert_eq!(sdv_trial_start(1_000), 0);
    sdv_step(10);
    let o = out();
    assert_eq!(o[TRIAL_STATE], 1.0, "running");
    assert_eq!(o[TRIAL_REMAINING_MS], 900.0);
    assert_eq!(o[CONFIG_SEQ], 1.0);
    step_seconds(1.0);
    assert_eq!(out()[TRIAL_STATE], 2.0, "passed");
    assert!(
        config_json().contains("\"max_accel_mps2\": 3.0"),
        "the update stays"
    );
    assert_eq!(sdv_nvm_slot_status(0), 1);

    // --- an unhealthy FIRST update falls back to the factory defaults
    factory_reset();
    commit(STRICT);
    sdv_boot();
    set_scenario(BUS_OUTAGE);
    sdv_reset();
    assert!(config_json().contains("\"heartbeat_timeout_ms\": 200"));
    assert_eq!(sdv_trial_start(5_000), 0);
    step_seconds(0.9);
    assert_eq!(out()[TRIAL_STATE], 1.0, "the outage has not started yet");
    step_seconds(1.5);
    let o = out();
    assert_eq!(o[TRIAL_STATE], 3.0, "rolled back");
    assert!(
        o[TRIAL_REASON] == 1.0 || o[TRIAL_REASON] == 2.0,
        "a reason is reported"
    );
    assert!(
        config_json().contains("\"heartbeat_timeout_ms\": 1000"),
        "the factory defaults run again"
    );
    assert_eq!(
        sdv_nvm_slot_status(0),
        -1,
        "the failed config is gone from the NVM"
    );
    assert_eq!(o[CONFIG_SEQ], 0.0, "generation 0 = factory defaults");
    // the system is healthy again with the old config, even through the same outage
    step_seconds(3.0);
    let o = out();
    assert_eq!(o[14], 0.0, "system state normal");
    assert_eq!(o[TRIAL_STATE], 3.0, "the verdict stays visible");

    // --- an unhealthy update goes back to the PREVIOUS config, not to the defaults
    factory_reset();
    commit(r#"{"vehicle_spec":{"max_accel_mps2":3.0}}"#);
    commit(STRICT);
    sdv_boot();
    set_scenario(BUS_OUTAGE);
    sdv_reset();
    assert_eq!(sdv_trial_start(5_000), 0);
    step_seconds(3.0);
    assert_eq!(out()[TRIAL_STATE], 3.0);
    let c = config_json();
    assert!(
        c.contains("\"max_accel_mps2\": 3.0") && c.contains("\"heartbeat_timeout_ms\": 1000"),
        "{c}"
    );
    assert_eq!(sdv_nvm_slot_status(1), -1, "no backup is left");
    assert!(
        sdv_nvm_slot_status(0) > 1,
        "the previous config is a new, newer record"
    );

    // --- the driver's own E-Stop is not the config's fault
    factory_reset();
    commit(r#"{"vehicle_spec":{"max_accel_mps2":3.0}}"#);
    sdv_boot();
    set_scenario(
        r#"{"steps":[{"until_s":60,"throttle":0.5}],
        "events":[{"at_s":0.2,"action":"estop"},{"at_s":0.7,"action":"estop_release"}]}"#,
    );
    sdv_reset();
    assert_eq!(sdv_trial_start(1_500), 0);
    step_seconds(2.0);
    assert_eq!(
        out()[TRIAL_STATE],
        2.0,
        "passed even though the system was in EStop for a while"
    );
    assert!(config_json().contains("\"max_accel_mps2\": 3.0"));

    // --- booting ends any trial; cancel does too
    factory_reset();
    assert_eq!(sdv_trial_start(5_000), 0);
    sdv_boot();
    sdv_step(1);
    assert_eq!(out()[TRIAL_STATE], 0.0, "a boot starts without a trial");
    assert_eq!(sdv_trial_start(5_000), 0);
    sdv_trial_cancel();
    sdv_step(1);
    assert_eq!(out()[TRIAL_STATE], 0.0);

    // --- the config log tells the story (generation, simulated time, reason)
    factory_reset();
    commit(STRICT);
    sdv_boot();
    set_scenario(BUS_OUTAGE);
    sdv_reset();
    assert_eq!(sdv_trial_start(5_000), 0);
    step_seconds(3.0);
    sdv_config_log_json();
    let log = buf();
    for needle in [
        "\"commit\"",
        "\"boot\"",
        "\"trial_started\"",
        "\"trial_failed\"",
        "\"rolled_back\"",
    ] {
        assert!(log.contains(needle), "log should contain {needle}: {log}");
    }
    let parsed: serde_json::Value = serde_json::from_str(&log).expect("the log is JSON");
    let failed = parsed
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event"] == "trial_failed")
        .expect("a trial_failed entry");
    assert!(
        failed["t_s"].as_f64().unwrap() > 1.0,
        "the simulated time of the failure is logged"
    );
    assert!(
        failed["detail"].as_str().unwrap().contains("estop")
            || failed["detail"].as_str().unwrap().contains("degraded")
    );
}
