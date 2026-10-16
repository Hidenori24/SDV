//! The C ABI used by the browser. It keeps global state, so everything is one sequential test.

use sdv_core::*;

fn out() -> Vec<f32> {
    let n = sdv_out_len() as usize;
    unsafe { std::slice::from_raw_parts(sdv_out_ptr(), n).to_vec() }
}

#[test]
fn c_api_exposes_bus_faults_vcu_stall_and_failsafe_flags() {
    assert_eq!(sdv_out_len(), 28);

    sdv_reset();
    sdv_use_scenario(0);
    sdv_set_input(1.0, 0.0, 0.0);
    sdv_step(200);
    assert!(out()[4] > 1.0, "speed");
    assert_eq!(&out()[19..22], &[0.0, 0.0, 0.0], "no failsafe while healthy");

    // vehicle bus down: after > 100 ms every ECU enters E-Stop behaviour by itself (bit 1)
    sdv_set_bus(0, 0, 1);
    sdv_step(15);
    assert_eq!(&out()[19..22], &[2.0, 2.0, 2.0]);
    // after > 200 ms the DriverInput timeout (bit 0) is set too
    sdv_step(10);
    assert_eq!(&out()[19..22], &[3.0, 3.0, 3.0]);
    // bus back: the ECUs recover on their own
    sdv_set_bus(0, 0, 0);
    sdv_step(10);
    assert_eq!(&out()[19..22], &[0.0, 0.0, 0.0]);

    // VCU stall: the ECUs fail safe and the car stops
    sdv_reset();
    sdv_set_input(1.0, 0.0, 0.0);
    sdv_step(300);
    sdv_set_vcu_stall(1);
    sdv_step(500);
    assert_eq!(out()[4], 0.0, "stopped");
    assert_eq!(out()[19 + 0] as u32 & 2, 2);

    // reset clears the injected faults
    sdv_reset();
    sdv_set_input(1.0, 0.0, 0.0);
    sdv_step(50);
    assert!(out()[4] > 0.0);
    assert_eq!(&out()[19..22], &[0.0, 0.0, 0.0]);
}
