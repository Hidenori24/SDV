#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

/**
 * @brief Vehicle Model Parameters
 *
 * Defaults are the values the VehicleDynamics SWC runs with (toy-scale placeholders).
 */
struct VehicleParams {
  float wheel_radius_m = 0.03f;    ///< 3 cm
  float wheelbase_m = 0.20f;       ///< 20 cm (kinematic bicycle model)
  float linear_drag = 0.15f;       ///< simple resistance coefficient (1/s)
  float max_speed_mps = 3.0f;      ///< speed cap
  float estop_decel_mps2 = 6.0f;   ///< extra forced deceleration while E-Stop
};

/**
 * @brief Vehicle state
 *
 * Sign convention: yaw / yaw_rate are positive counter-clockwise (a LEFT turn);
 * the vehicle faces +x at yaw = 0.
 */
struct VehicleState {
  float t = 0.0f;
  float x = 0.0f;           // m
  float y = 0.0f;           // m
  float yaw = 0.0f;         // rad
  float v = 0.0f;           // m/s
  float yaw_rate = 0.0f;    // rad/s
  float wheel_omega = 0.0f; // rad/s
};

inline float Resist(float v, const VehicleParams& p) {
  return p.linear_drag * v;
}

/**
 * @brief Longitudinal dynamics: speed, wheel speed and time (pure function)
 *
 * v is clamped to [0, max_speed]. While estop is true an extra forced deceleration is applied.
 */
inline VehicleState StepLongitudinal(
    const VehicleState& s,
    float dt,
    float drive_accel_cmd,
    float brake_decel_cmd,
    bool estop,
    const VehicleParams& p)
{
  VehicleState out = s;

  float accel = drive_accel_cmd - brake_decel_cmd - Resist(out.v, p);
  if (estop) accel -= p.estop_decel_mps2;

  out.v = std::clamp(out.v + accel * dt, 0.0f, p.max_speed_mps);

  float r = std::max(p.wheel_radius_m, 1e-4f);
  out.wheel_omega = out.v / r;

  out.t += dt;
  return out;
}

/**
 * @brief Lateral dynamics: kinematic bicycle model (pure function)
 *
 * yaw_rate = v / L * tan(steer); yaw and position are integrated with the (already updated) v.
 * A positive steer angle turns LEFT. A stationary vehicle neither turns nor moves.
 * Only yaw_rate, yaw, x and y are written.
 */
inline VehicleState StepLateral(
    const VehicleState& s,
    float dt,
    float steer_angle_cmd,
    const VehicleParams& p)
{
  VehicleState out = s;

  const float L = std::max(p.wheelbase_m, 1e-3f);
  out.yaw_rate = (out.v / L) * std::tan(steer_angle_cmd);
  out.yaw = out.yaw + out.yaw_rate * dt;

  out.x = out.x + out.v * std::cos(out.yaw) * dt;
  out.y = out.y + out.v * std::sin(out.yaw) * dt;
  return out;
}

/**
 * @brief One full vehicle step: longitudinal, then lateral (what the SWC calls every 10 ms)
 */
inline VehicleState StepVehicle(
    const VehicleState& s,
    float dt,
    float drive_accel_cmd,
    float brake_decel_cmd,
    float steer_angle_cmd,
    bool estop,
    const VehicleParams& p)
{
  return StepLateral(StepLongitudinal(s, dt, drive_accel_cmd, brake_decel_cmd, estop, p), dt, steer_angle_cmd, p);
}

} // namespace Model
