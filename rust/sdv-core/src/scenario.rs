//! Scenario (ADR-0003 step 5): driver inputs over time plus timed events, as JSON.
//!
//! ```json
//! { "schema": 1, "name": "demo",
//!   "steps":  [ {"until_s": 2.0, "throttle": 0.6}, {"until_s": 5.0, "throttle": 0.2, "steer": 0.2} ],
//!   "after":  { "brake": 0.0 },
//!   "events": [ {"at_s": 3.0, "action": "stall", "target": "steering"} ] }
//! ```
//! - the input at time `t` is the first step with `t < until_s`, else `after` (all zero by default)
//! - events fire once when the simulated time (the plant's clock, which freezes if the plant is stalled)
//!   reaches `at_s`; they are what `sdv_sim --stall` does on the C++ side, plus E-Stop and bus faults

use crate::config::ConfigError;
use crate::rte::DriverInput;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_STEPS: usize = 64;
pub const MAX_EVENTS: usize = 64;
pub const MAX_TIME_S: f32 = 3600.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Level {
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub until_s: f32,
    #[serde(default)]
    pub throttle: f32,
    #[serde(default)]
    pub brake: f32,
    #[serde(default)]
    pub steer: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Stop executing `target` (like `sdv_sim --stall`).
    Stall,
    Resume,
    /// External E-Stop request / release.
    Estop,
    EstopRelease,
    /// Vehicle bus down / up.
    BusDown,
    BusUp,
    /// Stall / resume the VCU.
    VcuStall,
    VcuResume,
}

impl Action {
    fn needs_target(self) -> bool {
        matches!(self, Action::Stall | Action::Resume)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Engine,
    Brake,
    Steering,
    Vehicledynamics,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub at_s: f32,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scenario {
    pub schema: u32,
    pub name: String,
    pub steps: Vec<Step>,
    pub after: Level,
    pub events: Vec<Event>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self { schema: SCHEMA_VERSION, name: String::new(), steps: Vec::new(), after: Level::default(), events: Vec::new() }
    }
}

impl Scenario {
    /// The built-in demo scenario (identical to the C++ DriverInputSWC).
    pub fn demo() -> Self {
        let step = |until_s, throttle, brake, steer| Step { until_s, throttle, brake, steer };
        Self {
            name: "demo".into(),
            steps: vec![step(2.0, 0.6, 0.0, 0.0), step(5.0, 0.2, 0.0, 0.2), step(7.0, 0.0, 0.6, 0.0)],
            ..Self::default()
        }
    }

    /// Driver input at simulated time `t` (clamped to the valid ranges).
    pub fn input_at(&self, t: f32) -> DriverInput {
        let (throttle, brake, steer) = match self.steps.iter().find(|s| t < s.until_s) {
            Some(s) => (s.throttle, s.brake, s.steer),
            None => (self.after.throttle, self.after.brake, self.after.steer),
        };
        DriverInput {
            throttle: throttle.clamp(0.0, 1.0),
            brake: brake.clamp(0.0, 1.0),
            steer: steer.clamp(-1.0, 1.0),
        }
    }

    /// Events sorted by time (stable, so equal times keep their order).
    pub fn sorted_events(&self) -> Vec<Event> {
        let mut v = self.events.clone();
        v.sort_by(|a, b| a.at_s.partial_cmp(&b.at_s).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}

/// Parse and validate. On error only the full list of problems is returned.
pub fn parse(json: &str) -> Result<Scenario, Vec<ConfigError>> {
    let s: Scenario = serde_json::from_str(json)
        .map_err(|e| vec![ConfigError { path: "$".into(), reason: e.to_string() }])?;
    let errors = validate(&s);
    if errors.is_empty() {
        Ok(s)
    } else {
        Err(errors)
    }
}

pub fn to_json(s: &Scenario) -> String {
    serde_json::to_string_pretty(s).expect("scenario is always serializable")
}

fn level_errors(path: &str, throttle: f32, brake: f32, steer: f32, out: &mut Vec<ConfigError>) {
    let mut chk = |name: &str, v: f32, lo: f32, hi: f32| {
        if !v.is_finite() || v < lo || v > hi {
            out.push(ConfigError { path: format!("{path}.{name}"), reason: format!("must be in [{lo}, {hi}] (got {v})") });
        }
    };
    chk("throttle", throttle, 0.0, 1.0);
    chk("brake", brake, 0.0, 1.0);
    chk("steer", steer, -1.0, 1.0);
}

pub fn validate(s: &Scenario) -> Vec<ConfigError> {
    let mut out = Vec::new();
    let mut err = |path: String, reason: String| out.push(ConfigError { path, reason });
    if s.schema != SCHEMA_VERSION {
        err("schema".into(), format!("unsupported schema version {} (supported: {SCHEMA_VERSION})", s.schema));
    }
    if s.steps.len() > MAX_STEPS {
        err("steps".into(), format!("at most {MAX_STEPS} steps (got {})", s.steps.len()));
    }
    if s.events.len() > MAX_EVENTS {
        err("events".into(), format!("at most {MAX_EVENTS} events (got {})", s.events.len()));
    }
    let mut prev = 0.0f32;
    for (i, st) in s.steps.iter().enumerate() {
        if !st.until_s.is_finite() || st.until_s <= prev || st.until_s > MAX_TIME_S {
            err(
                format!("steps[{i}].until_s"),
                format!("must be in ({prev}, {MAX_TIME_S}] and larger than the previous step (got {})", st.until_s),
            );
        } else {
            prev = st.until_s;
        }
    }
    drop(err);
    for (i, st) in s.steps.iter().enumerate() {
        level_errors(&format!("steps[{i}]"), st.throttle, st.brake, st.steer, &mut out);
    }
    level_errors("after", s.after.throttle, s.after.brake, s.after.steer, &mut out);
    for (i, e) in s.events.iter().enumerate() {
        if !e.at_s.is_finite() || e.at_s < 0.0 || e.at_s > MAX_TIME_S {
            out.push(ConfigError { path: format!("events[{i}].at_s"), reason: format!("must be in [0, {MAX_TIME_S}] (got {})", e.at_s) });
        }
        if e.action.needs_target() != e.target.is_some() {
            let reason = if e.action.needs_target() { "required for stall / resume" } else { "only allowed for stall / resume" };
            out.push(ConfigError { path: format!("events[{i}].target"), reason: reason.into() });
        }
    }
    out
}
