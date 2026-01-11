#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

struct VehicleParams {
  float wheel_radius_m = 0.03f;
  float linear_drag = 0.0f;        // 最初は0で良い
  float max_speed_mps = 3.0f;
  float estop_decel_mps2 = 6.0f;
};

struct VehicleState {
  float t = 0.0f;
  float v = 0.0f;
  float wheel_omega = 0.0f; // rad/s
};

inline float Resist(float v, const VehicleParams& p) {
  return p.linear_drag * v;
}

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

} // namespace Model
