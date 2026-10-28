//! Simulated vehicle platform for exercising the same ECU executor used by a real platform.

use std::convert::Infallible;

use crate::config::Config;
use crate::model::VehicleState;
use crate::platform::{PlatformInputs, VehiclePlatform};
use crate::rte::{ActuatorCmd, DriverInput, Rte};
use crate::runtime::VehicleActuation;
use crate::swc::VehicleDynamicsSwc;

const STEP_SECONDS: f32 = 0.010;

pub struct SimulatedPlatform {
    rte: Rte,
    plant: VehicleDynamicsSwc,
    driver: DriverInput,
    estop: bool,
}

impl SimulatedPlatform {
    pub fn new(config: &Config) -> Self {
        Self {
            rte: Rte::default(),
            plant: VehicleDynamicsSwc::new(config.vehicle_params()),
            driver: DriverInput::default(),
            estop: false,
        }
    }

    pub fn set_driver_input(&mut self, input: DriverInput) {
        self.driver = input;
    }
    pub fn set_estop(&mut self, estop: bool) {
        self.estop = estop;
    }
    pub fn state(&self) -> VehicleState {
        self.rte.state
    }
}

impl VehiclePlatform for SimulatedPlatform {
    type Error = Infallible;

    fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
        Ok(PlatformInputs {
            driver: self.driver,
            vehicle: self.rte.state,
            estop: self.estop,
        })
    }

    fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
        self.rte.cmd = actuation.command;
        self.rte.safety = actuation.safety;
        self.plant.step_10ms(&mut self.rte, STEP_SECONDS);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.rte.cmd = ActuatorCmd::default();
        self.rte.safety.estop = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::VehicleExecutor;
    use crate::runtime::VehicleRuntime;
    use crate::sim::Sim;

    #[test]
    fn executor_and_simulator_follow_same_trajectory() {
        let config = Config::default();
        let mut executor = VehicleExecutor::new(
            VehicleRuntime::new(config.clone()),
            SimulatedPlatform::new(&config),
        );
        let mut reference = Sim::with_config(config);
        let input = DriverInput {
            throttle: 0.5,
            brake: 0.1,
            steer: 0.2,
        };
        executor.platform_mut().set_driver_input(input);
        *reference.input_mut() = input;
        for _ in 0..300 {
            let expected = reference.step_10ms();
            let actuation = executor.cycle().unwrap();
            assert_eq!(actuation.command, expected.cmd);
            assert_eq!(executor.platform().state(), expected.state);
        }
    }
}
