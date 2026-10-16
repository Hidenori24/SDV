//! Scenario through the C ABI (global state => one sequential test per binary).

use sdv_core::*;

fn buf() -> String {
    let n = sdv_buf_len() as usize;
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(sdv_buf_ptr(), n) }).to_string()
}

fn out() -> Vec<f32> {
    unsafe { std::slice::from_raw_parts(sdv_out_ptr(), sdv_out_len() as usize) }.to_vec()
}

fn set_scenario(json: &str) -> i32 {
    let b = json.as_bytes();
    let p = sdv_alloc(b.len() as i32);
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len()) };
    let r = sdv_set_scenario(p, b.len() as i32);
    sdv_free(p, b.len() as i32);
    r
}

#[test]
fn scenario_and_state_flags_through_the_c_api() {
    sdv_reset();
    sdv_demo_scenario_json();
    assert!(buf().contains("\"until_s\": 2.0") && buf().contains("\"name\": \"demo\""));
    sdv_scenario_json();
    assert!(buf().contains("\"name\": \"demo\""), "the running scenario is the demo by default");

    // invalid scenario: rejected with every problem, the running scenario is unchanged
    assert_eq!(set_scenario(r#"{"steps":[{"until_s":1.0,"throttle":9.0}],"events":[{"at_s":1.0,"action":"stall"}]}"#), 1);
    let e = buf();
    assert!(e.contains("steps[0].throttle") && e.contains("events[0].target"), "{e}");
    sdv_scenario_json();
    assert!(buf().contains("\"name\": \"demo\""));

    // a scenario with events: steering stalls at 1 s, the bus goes down at 2 s, the VCU stalls at 3 s
    assert_eq!(
        set_scenario(
            r#"{"name":"faults","steps":[{"until_s":10.0,"throttle":0.8}],
                "events":[{"at_s":1.0,"action":"stall","target":"steering"},
                          {"at_s":2.0,"action":"bus_down"},
                          {"at_s":3.0,"action":"vcu_stall"}]}"#
        ),
        0
    );
    sdv_scenario_json();
    assert!(buf().contains("\"name\": \"faults\""));
    sdv_reset();
    sdv_use_scenario(1);
    assert_eq!(sdv_out_len(), 28);
    sdv_step(50);
    let o = out();
    assert_eq!(&o[22..28], &[0.0; 6], "nothing yet at 0.5 s");
    assert!(o[7] > 0.5, "scenario throttle");
    sdv_step(100);
    let o = out();
    assert_eq!(o[24], 1.0, "steering stalled (flags: engine, brake, steering, plant, vcu, bus)");
    assert_eq!(&o[22..24], &[0.0, 0.0]);
    sdv_step(100);
    assert_eq!(out()[27], 1.0, "bus down");
    sdv_step(100);
    assert_eq!(out()[26], 1.0, "vcu stalled");

    // reset replays the scenario from the start and clears the faults
    sdv_reset();
    sdv_step(10);
    assert_eq!(&out()[22..28], &[0.0; 6]);
    assert_eq!(out()[4] > 0.0, true);
}
