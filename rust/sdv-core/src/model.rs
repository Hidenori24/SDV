//! Pure-function Model layer. Mirrors `src/model/*.h` of the C++ implementation.

#[derive(Clone, Copy, Debug)]
pub struct EngineParams {
    pub max_accel_mps2: f32,
}
impl Default for EngineParams {
    fn default() -> Self {
        Self { max_accel_mps2: 2.0 }
    }
}

pub fn compute_drive_accel(throttle_0_1: f32, estop: bool, p: &EngineParams) -> f32 {
    if estop {
        return 0.0;
    }
    throttle_0_1.clamp(0.0, 1.0) * p.max_accel_mps2
}

#[derive(Clone, Copy, Debug)]
pub struct BrakeParams {
    pub max_decel_mps2: f32,
    pub estop_max_decel_mps2: f32,
}
impl Default for BrakeParams {
    fn default() -> Self {
        Self { max_decel_mps2: 4.0, estop_max_decel_mps2: 4.0 }
    }
}

pub fn compute_brake_decel(brake_0_1: f32, estop: bool, p: &BrakeParams) -> f32 {
    if estop {
        return p.estop_max_decel_mps2;
    }
    brake_0_1.clamp(0.0, 1.0) * p.max_decel_mps2
}

#[derive(Clone, Copy, Debug)]
pub struct SteeringParams {
    pub max_steer_angle_rad: f32,
    pub time_constant_s: f32,
}
impl Default for SteeringParams {
    fn default() -> Self {
        Self { max_steer_angle_rad: 0.40, time_constant_s: 0.15 }
    }
}

pub fn compute_steer_target(steer_norm: f32, estop: bool, p: &SteeringParams) -> f32 {
    if estop {
        return 0.0;
    }
    let steer = steer_norm.clamp(-1.0, 1.0);
    (steer * p.max_steer_angle_rad).clamp(-p.max_steer_angle_rad, p.max_steer_angle_rad)
}

/// First-order lag, exact discretization (no overshoot for any dt).
pub fn step_steering_dynamics(current: f32, target: f32, dt_s: f32, p: &SteeringParams) -> f32 {
    let target = target.clamp(-p.max_steer_angle_rad, p.max_steer_angle_rad);
    if dt_s <= 0.0 {
        return current;
    }
    let tau = p.time_constant_s.max(1e-3);
    let alpha = 1.0 - (-dt_s / tau).exp();
    current + alpha * (target - current)
}

#[derive(Clone, Copy, Debug)]
pub struct VehicleParams {
    pub wheel_radius_m: f32,
    pub wheelbase_m: f32,
    pub linear_drag: f32,
    pub max_speed_mps: f32,
    pub estop_decel_mps2: f32,
}
impl Default for VehicleParams {
    // Matches the parameters actually used by the C++ VehicleDynamicsSWC.
    fn default() -> Self {
        Self {
            wheel_radius_m: 0.03,
            wheelbase_m: 0.20,
            linear_drag: 0.15,
            max_speed_mps: 3.0,
            estop_decel_mps2: 6.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleState {
    pub t: f32,
    pub x: f32,
    pub y: f32,
    pub yaw: f32,
    pub v: f32,
    pub yaw_rate: f32,
    pub wheel_omega: f32,
}

/// Longitudinal dynamics + kinematic bicycle model for one step.
pub fn step_vehicle(
    s: &VehicleState,
    dt: f32,
    drive_accel_cmd: f32,
    brake_decel_cmd: f32,
    steer_angle_cmd: f32,
    estop: bool,
    p: &VehicleParams,
) -> VehicleState {
    let mut o = *s;

    let mut accel = drive_accel_cmd - brake_decel_cmd - p.linear_drag * o.v;
    if estop {
        accel -= p.estop_decel_mps2;
    }
    o.v = (o.v + accel * dt).clamp(0.0, p.max_speed_mps);

    let l = p.wheelbase_m.max(1e-3);
    o.yaw_rate = (o.v / l) * steer_angle_cmd.tan();
    o.yaw += o.yaw_rate * dt;

    o.x += o.v * o.yaw.cos() * dt;
    o.y += o.v * o.yaw.sin() * dt;

    o.wheel_omega = o.v / p.wheel_radius_m.max(1e-4);
    o.t += dt;
    o
}

// ---- Safety logic (mirrors `src/model/safety_logic.h`) ----

/// Safety state (values match the system_state signal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SafetyState {
    #[default]
    Normal = 0,
    Degraded = 1,
    EStop = 2,
}

#[derive(Clone, Copy, Debug)]
pub struct SafetyParams {
    /// A monitored SWC is "dead" when silent for longer than this.
    pub heartbeat_timeout_ms: u32,
}
impl Default for SafetyParams {
    fn default() -> Self {
        Self { heartbeat_timeout_ms: 1000 }
    }
}

/// Per-SWC liveness verdict (true = alive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeartbeatStatus {
    pub engine_ok: bool,
    pub brake_ok: bool,
    pub steering_ok: bool,
    pub vehicledynamics_ok: bool,
}
impl Default for HeartbeatStatus {
    fn default() -> Self {
        Self { engine_ok: true, brake_ok: true, steering_ok: true, vehicledynamics_ok: true }
    }
}

/// Watchdog state for one monitored SWC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeartbeatMonitor {
    /// Alive counter observed at the previous check.
    pub last_count: u64,
    /// Time during which the counter did not advance.
    pub silent_ms: u32,
}

/// The counter advancing resets the silence timer; otherwise silence grows by `dt_ms`.
/// Returns true while the SWC is considered alive (`silent_ms <= timeout`).
pub fn update_heartbeat(m: &mut HeartbeatMonitor, alive_count: u64, dt_ms: u32, p: &SafetyParams) -> bool {
    if alive_count != m.last_count {
        m.last_count = alive_count;
        m.silent_ms = 0;
    } else {
        m.silent_ms += dt_ms;
    }
    m.silent_ms <= p.heartbeat_timeout_ms
}

pub fn count_failed(hb: &HeartbeatStatus) -> u32 {
    [hb.engine_ok, hb.brake_ok, hb.steering_ok, hb.vehicledynamics_ok]
        .iter()
        .filter(|ok| !**ok)
        .count() as u32
}

/// 0 dead -> Normal, 1 dead -> Degraded, 2 or more dead -> EStop.
pub fn compute_safety_state(hb: &HeartbeatStatus) -> SafetyState {
    match count_failed(hb) {
        0 => SafetyState::Normal,
        1 => SafetyState::Degraded,
        _ => SafetyState::EStop,
    }
}
