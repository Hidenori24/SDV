//! Config (ADR-0003): calibration parameters of the SWCs / BSW as JSON.
//!
//! - missing fields use the defaults (identical to the previous hard-coded constants)
//! - unknown fields are rejected (a typo must not be ignored silently)
//! - a config with any out-of-range value is rejected as a whole; every problem is listed

use crate::failsafe::FailsafeParams;
use crate::model::*;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Fail-safe timeouts in milliseconds (converted to 10 ms ticks for the ECUs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FailsafeConfig {
    pub driver_timeout_ms: u32,
    pub safety_timeout_ms: u32,
}

impl Default for FailsafeConfig {
    fn default() -> Self {
        Self { driver_timeout_ms: 200, safety_timeout_ms: 100 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub engine: EngineParams,
    pub brake: BrakeParams,
    pub steering: SteeringParams,
    pub vehicle: VehicleParams,
    pub safety: SafetyParams,
    pub failsafe: FailsafeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            engine: EngineParams::default(),
            brake: BrakeParams::default(),
            steering: SteeringParams::default(),
            vehicle: VehicleParams::default(),
            safety: SafetyParams::default(),
            failsafe: FailsafeConfig::default(),
        }
    }
}

impl Config {
    /// Fail-safe timeouts as whole 10 ms ticks (rounded up).
    pub fn failsafe_params(&self) -> FailsafeParams {
        let ticks = |ms: u32| ms.div_ceil(10);
        FailsafeParams {
            driver_timeout_ticks: ticks(self.failsafe.driver_timeout_ms),
            safety_timeout_ticks: ticks(self.failsafe.safety_timeout_ms),
        }
    }
}

/// One problem found in a config: where (`engine.max_accel_mps2`, `$` for the whole document) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub path: String,
    pub reason: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

/// Parse and validate. On error nothing is returned except the full list of problems.
pub fn parse(json: &str) -> Result<Config, Vec<ConfigError>> {
    let cfg: Config = serde_json::from_str(json)
        .map_err(|e| vec![ConfigError { path: "$".into(), reason: e.to_string() }])?;
    let errors = validate(&cfg);
    if errors.is_empty() {
        Ok(cfg)
    } else {
        Err(errors)
    }
}

pub fn to_json(cfg: &Config) -> String {
    serde_json::to_string_pretty(cfg).expect("config is always serializable")
}

struct Checker(Vec<ConfigError>);

impl Checker {
    fn err(&mut self, path: &str, reason: String) {
        self.0.push(ConfigError { path: path.into(), reason });
    }

    fn range_f32(&mut self, path: &str, v: f32, lo: f32, hi: f32) {
        if !v.is_finite() || v < lo || v > hi {
            self.err(path, format!("must be in [{lo}, {hi}] (got {v})"));
        }
    }

    fn range_u32(&mut self, path: &str, v: u32, lo: u32, hi: u32) {
        if v < lo || v > hi {
            self.err(path, format!("must be in [{lo}, {hi}] (got {v})"));
        }
    }
}

/// Range checks of ADR-0003.
pub fn validate(c: &Config) -> Vec<ConfigError> {
    let mut k = Checker(Vec::new());
    if c.schema != SCHEMA_VERSION {
        k.err("schema", format!("unsupported schema version {} (supported: {SCHEMA_VERSION})", c.schema));
    }
    k.range_f32("engine.max_accel_mps2", c.engine.max_accel_mps2, 0.0, 10.0);
    k.range_f32("brake.max_decel_mps2", c.brake.max_decel_mps2, 0.1, 10.0);
    k.range_f32("brake.estop_max_decel_mps2", c.brake.estop_max_decel_mps2, 0.1, 15.0);
    if c.brake.estop_max_decel_mps2.is_finite()
        && c.brake.max_decel_mps2.is_finite()
        && c.brake.estop_max_decel_mps2 < c.brake.max_decel_mps2
    {
        k.err(
            "brake.estop_max_decel_mps2",
            format!("must not be smaller than brake.max_decel_mps2 ({})", c.brake.max_decel_mps2),
        );
    }
    k.range_f32("steering.max_steer_angle_rad", c.steering.max_steer_angle_rad, 0.05, 0.6);
    k.range_f32("steering.time_constant_s", c.steering.time_constant_s, 0.01, 2.0);
    k.range_f32("vehicle.wheel_radius_m", c.vehicle.wheel_radius_m, 0.005, 1.0);
    k.range_f32("vehicle.wheelbase_m", c.vehicle.wheelbase_m, 0.05, 5.0);
    k.range_f32("vehicle.linear_drag", c.vehicle.linear_drag, 0.0, 2.0);
    k.range_f32("vehicle.max_speed_mps", c.vehicle.max_speed_mps, 0.1, 50.0);
    k.range_f32("vehicle.estop_decel_mps2", c.vehicle.estop_decel_mps2, 0.0, 20.0);
    k.range_u32("safety.heartbeat_timeout_ms", c.safety.heartbeat_timeout_ms, 200, 10_000);
    k.range_u32("failsafe.driver_timeout_ms", c.failsafe.driver_timeout_ms, 50, 2000);
    k.range_u32("failsafe.safety_timeout_ms", c.failsafe.safety_timeout_ms, 30, 1000);
    k.0
}
