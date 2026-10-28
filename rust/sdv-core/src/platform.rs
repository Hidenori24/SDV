//! Boundary between virtual ECU tasks and a vehicle's input and output devices.

use crate::model::VehicleState;
use crate::rte::DriverInput;
use crate::runtime::{VehicleActuation, VehicleRuntime};

/// A consistent platform snapshot for one control cycle.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformInputs {
    pub driver: DriverInput,
    pub vehicle: VehicleState,
    pub estop: bool,
}

/// Platform adapters implement device access and a best-effort actuator stop.
/// `sample` must return an error when its inputs are stale or its I/O path is unavailable.
pub trait VehiclePlatform {
    type Error;
    fn sample(&mut self) -> Result<PlatformInputs, Self::Error>;
    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error>;
    fn stop(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq)]
pub enum CycleError<E> {
    Sample { source: E, stop_error: Option<E> },
    Apply { source: E, stop_error: Option<E> },
    InvalidInput { stop_error: Option<E> },
}

/// Connects the same ECU runtime to a simulated or physical platform.
pub struct VehicleExecutor<P> {
    runtime: VehicleRuntime,
    platform: P,
}

impl<P: VehiclePlatform> VehicleExecutor<P> {
    pub fn new(runtime: VehicleRuntime, platform: P) -> Self {
        Self { runtime, platform }
    }

    /// Run one cycle. On an I/O error or invalid input, immediately request a platform stop.
    pub fn cycle(&mut self) -> Result<VehicleActuation, CycleError<P::Error>> {
        let input = match self.platform.sample() {
            Ok(input) => input,
            Err(source) => {
                return Err(CycleError::Sample {
                    source,
                    stop_error: self.platform.stop().err(),
                })
            }
        };
        if !finite(&input) {
            return Err(CycleError::InvalidInput {
                stop_error: self.platform.stop().err(),
            });
        }
        self.runtime.set_estop_request(input.estop);
        let actuation = self.runtime.step_10ms(input.driver, input.vehicle, true);
        if let Err(source) = self.platform.apply(actuation) {
            return Err(CycleError::Apply {
                source,
                stop_error: self.platform.stop().err(),
            });
        }
        Ok(actuation)
    }

    pub fn stop(&mut self) -> Result<(), P::Error> {
        self.platform.stop()
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }
}

fn finite(input: &PlatformInputs) -> bool {
    let d = input.driver;
    let v = input.vehicle;
    [
        d.throttle,
        d.brake,
        d.steer,
        v.t,
        v.x,
        v.y,
        v.yaw,
        v.v,
        v.yaw_rate,
        v.wheel_omega,
    ]
    .iter()
    .all(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePlatform {
        sample_error: bool,
        apply_error: bool,
        invalid: bool,
        stops: u32,
        applied: u32,
    }
    impl VehiclePlatform for FakePlatform {
        type Error = &'static str;
        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            if self.sample_error {
                return Err("sample");
            }
            let mut inputs = PlatformInputs::default();
            if self.invalid {
                inputs.driver.throttle = f32::NAN;
            }
            Ok(inputs)
        }
        fn apply(&mut self, _: VehicleActuation) -> Result<(), Self::Error> {
            self.applied += 1;
            if self.apply_error {
                Err("apply")
            } else {
                Ok(())
            }
        }
        fn stop(&mut self) -> Result<(), Self::Error> {
            self.stops += 1;
            Ok(())
        }
    }

    #[test]
    fn sample_failure_and_invalid_values_stop_without_applying() {
        for platform in [
            FakePlatform {
                sample_error: true,
                ..Default::default()
            },
            FakePlatform {
                invalid: true,
                ..Default::default()
            },
        ] {
            let mut executor = VehicleExecutor::new(VehicleRuntime::default(), platform);
            assert!(executor.cycle().is_err());
            assert_eq!(executor.platform().stops, 1);
            assert_eq!(executor.platform().applied, 0);
        }
    }

    #[test]
    fn failed_output_requests_stop() {
        let mut executor = VehicleExecutor::new(
            VehicleRuntime::default(),
            FakePlatform {
                apply_error: true,
                ..Default::default()
            },
        );
        assert_eq!(
            executor.cycle(),
            Err(CycleError::Apply {
                source: "apply",
                stop_error: None
            })
        );
        assert_eq!(executor.platform().stops, 1);
    }
}
