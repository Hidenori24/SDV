//! Config (ADR-0003, revision 1 = schema 2): the car and the software, as JSON.
//!
//! Four groups:
//! - `vehicle_spec`: the car itself (dimensions, performance) - used by the plant and the ECU software
//! - `physics`: the world (gravity, tyre friction, running resistance) - used by the plant only
//! - `tuning`: the software's settings (E-Stop braking, Watchdog, fail-safe)
//! - derived values (`Config::derived`): computed from the above, read-only
//!
//! Missing fields use the defaults, unknown fields are rejected, and a config with any out-of-range value
//! is rejected as a whole (every problem is listed with its path). Schema 1 documents are converted
//! automatically.

use crate::failsafe::FailsafeParams;
use crate::model::*;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;

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

/// The car itself: dimensions and performance (data sheet or measurement).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VehicleSpec {
    pub wheelbase_m: f32,
    pub wheel_radius_m: f32,
    /// Mechanical limit of the road wheels.
    pub max_steer_angle_rad: f32,
    /// Steering servo lag (first order).
    pub steer_time_constant_s: f32,
    pub top_speed_mps: f32,
    pub max_accel_mps2: f32,
    pub max_decel_mps2: f32,
    /// Emergency stop ability.
    pub estop_decel_mps2: f32,
}

impl Default for VehicleSpec {
    fn default() -> Self {
        Self {
            wheelbase_m: 0.20,
            wheel_radius_m: 0.03,
            max_steer_angle_rad: 0.40,
            steer_time_constant_s: 0.15,
            top_speed_mps: 3.0,
            max_accel_mps2: 2.0,
            max_decel_mps2: 4.0,
            estop_decel_mps2: 6.0,
        }
    }
}

/// The world: values nature decides. Used by the plant only (on the real car nature supplies them).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Physics {
    pub gravity_mps2: f32,
    /// Depends on the surface (floor, carpet, asphalt).
    pub tire_friction_mu: f32,
    /// Simple linear running resistance.
    pub drag_coeff: f32,
}

impl Default for Physics {
    fn default() -> Self {
        Self { gravity_mps2: 9.81, tire_friction_mu: 0.6, drag_coeff: 0.15 }
    }
}

/// The software's settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tuning {
    pub estop_max_decel_mps2: f32,
    pub safety: SafetyParams,
    pub failsafe: FailsafeConfig,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { estop_max_decel_mps2: 4.0, safety: SafetyParams::default(), failsafe: FailsafeConfig::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub vehicle_spec: VehicleSpec,
    pub physics: Physics,
    pub tuning: Tuning,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            vehicle_spec: VehicleSpec::default(),
            physics: Physics::default(),
            tuning: Tuning::default(),
        }
    }
}

/// Values computed from spec + physics (never set by hand).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Derived {
    /// `L / tan(max steer)`
    pub min_turn_radius_m: f32,
    /// `mu * g`
    pub max_lateral_accel_mps2: f32,
    /// Above this speed full lock is limited by grip.
    pub critical_speed_mps: f32,
}

/// One point of the "what full lock does at this speed" curve.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TurnPoint {
    pub speed_mps: f32,
    pub max_steer_rad: f32,
    pub yaw_rate_rad_s: f32,
    pub turn_radius_m: f32,
    pub lateral_accel_mps2: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DerivedReport {
    pub derived: Derived,
    pub turn_curve: Vec<TurnPoint>,
}

impl Config {
    fn wheelbase(&self) -> f32 {
        self.vehicle_spec.wheelbase_m.max(1e-3)
    }

    pub fn derived(&self) -> Derived {
        let min_turn_radius_m = self.wheelbase() / self.vehicle_spec.max_steer_angle_rad.tan();
        let max_lateral_accel_mps2 = self.physics.tire_friction_mu * self.physics.gravity_mps2;
        Derived {
            min_turn_radius_m,
            max_lateral_accel_mps2,
            critical_speed_mps: (max_lateral_accel_mps2 * min_turn_radius_m).sqrt(),
        }
    }

    /// Largest steering angle that takes effect at speed `v`: `min(max steer, atan(L * mu * g / v^2))`.
    pub fn max_steer_at_speed(&self, v: f32) -> f32 {
        if v <= 0.0 {
            return self.vehicle_spec.max_steer_angle_rad;
        }
        let a = (self.physics.tire_friction_mu * self.physics.gravity_mps2).max(1e-3);
        self.vehicle_spec.max_steer_angle_rad.min((self.wheelbase() * a / (v * v)).atan())
    }

    /// Yaw rate the plant produces at speed `v` with the steering at full lock.
    pub fn yaw_rate_at_full_lock(&self, v: f32) -> f32 {
        (v / self.wheelbase()) * self.max_steer_at_speed(v).tan()
    }

    /// Derived values plus the full-lock curve over the speed range (for display).
    pub fn derived_report(&self) -> DerivedReport {
        let top = self.vehicle_spec.top_speed_mps.max(0.1);
        let n = 8;
        let turn_curve = (1..=n)
            .map(|i| {
                let v = top * i as f32 / n as f32;
                let yaw = self.yaw_rate_at_full_lock(v);
                TurnPoint {
                    speed_mps: v,
                    max_steer_rad: self.max_steer_at_speed(v),
                    yaw_rate_rad_s: yaw,
                    turn_radius_m: if yaw.abs() > 1e-6 { v / yaw } else { f32::INFINITY },
                    lateral_accel_mps2: v * yaw,
                }
            })
            .collect();
        DerivedReport { derived: self.derived(), turn_curve }
    }

    pub fn derived_json(&self) -> String {
        serde_json::to_string_pretty(&self.derived_report()).expect("report is always serializable")
    }

    // ---- the parameter structs of the individual models ----

    pub fn engine_params(&self) -> EngineParams {
        EngineParams { max_accel_mps2: self.vehicle_spec.max_accel_mps2 }
    }

    pub fn brake_params(&self) -> BrakeParams {
        BrakeParams { max_decel_mps2: self.vehicle_spec.max_decel_mps2, estop_max_decel_mps2: self.tuning.estop_max_decel_mps2 }
    }

    pub fn steering_params(&self) -> SteeringParams {
        SteeringParams {
            max_steer_angle_rad: self.vehicle_spec.max_steer_angle_rad,
            time_constant_s: self.vehicle_spec.steer_time_constant_s,
        }
    }

    pub fn vehicle_params(&self) -> VehicleParams {
        VehicleParams {
            wheel_radius_m: self.vehicle_spec.wheel_radius_m,
            wheelbase_m: self.vehicle_spec.wheelbase_m,
            linear_drag: self.physics.drag_coeff,
            max_speed_mps: self.vehicle_spec.top_speed_mps,
            estop_decel_mps2: self.vehicle_spec.estop_decel_mps2,
            tire_friction_mu: self.physics.tire_friction_mu,
            gravity_mps2: self.physics.gravity_mps2,
        }
    }

    pub fn safety_params(&self) -> SafetyParams {
        self.tuning.safety
    }

    /// Fail-safe timeouts as whole 10 ms ticks (rounded up).
    pub fn failsafe_params(&self) -> FailsafeParams {
        let ticks = |ms: u32| ms.div_ceil(10);
        FailsafeParams {
            driver_timeout_ticks: ticks(self.tuning.failsafe.driver_timeout_ms),
            safety_timeout_ticks: ticks(self.tuning.failsafe.safety_timeout_ms),
        }
    }
}

/// One problem found in a config: where (`vehicle_spec.max_accel_mps2`, `$` for the whole document) and why.
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

// ---------------------------------------------------------------------------
// schema 1 (read-only: converted to schema 2)
// ---------------------------------------------------------------------------

const V1_KEYS: [&str; 6] = ["engine", "brake", "steering", "vehicle", "safety", "failsafe"];

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Engine {
    max_accel_mps2: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Brake {
    max_decel_mps2: Option<f32>,
    estop_max_decel_mps2: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Steering {
    max_steer_angle_rad: Option<f32>,
    time_constant_s: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Vehicle {
    wheel_radius_m: Option<f32>,
    wheelbase_m: Option<f32>,
    linear_drag: Option<f32>,
    max_speed_mps: Option<f32>,
    estop_decel_mps2: Option<f32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Safety {
    heartbeat_timeout_ms: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct V1Failsafe {
    driver_timeout_ms: Option<u32>,
    safety_timeout_ms: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigV1 {
    #[allow(dead_code)]
    schema: Option<u32>,
    engine: V1Engine,
    brake: V1Brake,
    steering: V1Steering,
    vehicle: V1Vehicle,
    safety: V1Safety,
    failsafe: V1Failsafe,
}

impl ConfigV1 {
    /// Missing fields keep the schema 2 defaults; the new schema 2 fields (gravity, friction) stay at their defaults.
    fn into_v2(self) -> Config {
        let mut c = Config::default();
        let (s, p, t) = (&mut c.vehicle_spec, &mut c.physics, &mut c.tuning);
        macro_rules! set {
            ($dst:expr, $src:expr) => {
                if let Some(v) = $src {
                    $dst = v;
                }
            };
        }
        set!(s.max_accel_mps2, self.engine.max_accel_mps2);
        set!(s.max_decel_mps2, self.brake.max_decel_mps2);
        set!(t.estop_max_decel_mps2, self.brake.estop_max_decel_mps2);
        set!(s.max_steer_angle_rad, self.steering.max_steer_angle_rad);
        set!(s.steer_time_constant_s, self.steering.time_constant_s);
        set!(s.wheel_radius_m, self.vehicle.wheel_radius_m);
        set!(s.wheelbase_m, self.vehicle.wheelbase_m);
        set!(p.drag_coeff, self.vehicle.linear_drag);
        set!(s.top_speed_mps, self.vehicle.max_speed_mps);
        set!(s.estop_decel_mps2, self.vehicle.estop_decel_mps2);
        set!(t.safety.heartbeat_timeout_ms, self.safety.heartbeat_timeout_ms);
        set!(t.failsafe.driver_timeout_ms, self.failsafe.driver_timeout_ms);
        set!(t.failsafe.safety_timeout_ms, self.failsafe.safety_timeout_ms);
        c
    }
}

fn whole_document_error(reason: impl Into<String>) -> Vec<ConfigError> {
    vec![ConfigError { path: "$".into(), reason: reason.into() }]
}

/// Parse and validate (schema 1 is converted). On error nothing is returned except the full list of problems.
pub fn parse(json: &str) -> Result<Config, Vec<ConfigError>> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| whole_document_error(e.to_string()))?;
    let obj = value.as_object().ok_or_else(|| whole_document_error("expected a JSON object"))?;

    let legacy = match obj.get("schema") {
        Some(v) => match v.as_u64() {
            Some(1) => true,
            Some(2) => false,
            _ => {
                return Err(vec![ConfigError {
                    path: "schema".into(),
                    reason: format!("unsupported schema version {v} (supported: 1 [converted], {SCHEMA_VERSION})"),
                }])
            }
        },
        None => V1_KEYS.iter().any(|k| obj.contains_key(*k)),
    };

    let cfg = if legacy {
        serde_json::from_value::<ConfigV1>(value).map_err(|e| whole_document_error(e.to_string()))?.into_v2()
    } else {
        serde_json::from_value::<Config>(value).map_err(|e| whole_document_error(e.to_string()))?
    };
    let errors = validate(&cfg);
    if errors.is_empty() {
        Ok(cfg)
    } else {
        Err(errors)
    }
}

/// Always writes schema 2.
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

/// Range checks of ADR-0003 (revision 1).
pub fn validate(c: &Config) -> Vec<ConfigError> {
    let mut k = Checker(Vec::new());
    if c.schema != SCHEMA_VERSION {
        k.err("schema", format!("unsupported schema version {} (supported: {SCHEMA_VERSION})", c.schema));
    }
    let (s, p, t) = (&c.vehicle_spec, &c.physics, &c.tuning);
    k.range_f32("vehicle_spec.wheelbase_m", s.wheelbase_m, 0.05, 5.0);
    k.range_f32("vehicle_spec.wheel_radius_m", s.wheel_radius_m, 0.005, 1.0);
    k.range_f32("vehicle_spec.max_steer_angle_rad", s.max_steer_angle_rad, 0.05, 0.6);
    k.range_f32("vehicle_spec.steer_time_constant_s", s.steer_time_constant_s, 0.01, 2.0);
    k.range_f32("vehicle_spec.top_speed_mps", s.top_speed_mps, 0.1, 50.0);
    k.range_f32("vehicle_spec.max_accel_mps2", s.max_accel_mps2, 0.0, 10.0);
    k.range_f32("vehicle_spec.max_decel_mps2", s.max_decel_mps2, 0.1, 10.0);
    k.range_f32("vehicle_spec.estop_decel_mps2", s.estop_decel_mps2, 0.0, 20.0);
    k.range_f32("physics.gravity_mps2", p.gravity_mps2, 1.0, 25.0);
    k.range_f32("physics.tire_friction_mu", p.tire_friction_mu, 0.05, 1.5);
    k.range_f32("physics.drag_coeff", p.drag_coeff, 0.0, 2.0);
    k.range_f32("tuning.estop_max_decel_mps2", t.estop_max_decel_mps2, 0.1, 15.0);
    if t.estop_max_decel_mps2.is_finite() && s.max_decel_mps2.is_finite() && t.estop_max_decel_mps2 < s.max_decel_mps2 {
        k.err(
            "tuning.estop_max_decel_mps2",
            format!("must not be smaller than vehicle_spec.max_decel_mps2 ({})", s.max_decel_mps2),
        );
    }
    k.range_u32("tuning.safety.heartbeat_timeout_ms", t.safety.heartbeat_timeout_ms, 200, 10_000);
    k.range_u32("tuning.failsafe.driver_timeout_ms", t.failsafe.driver_timeout_ms, 50, 2000);
    k.range_u32("tuning.failsafe.safety_timeout_ms", t.failsafe.safety_timeout_ms, 30, 1000);
    k.0
}
