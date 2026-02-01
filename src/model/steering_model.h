#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

struct SteeringParams {
  float max_steer_angle_rad = 0.40f; // ~23 deg
  float steer_tau_s = 0.15f;         // first-order lag
};

inline float StepSteerAngle(
    float steer_input,
    bool estop,
    float prev_angle,
    float dt_s,
    const SteeringParams& p)
{
  float target = 0.0f;
  if (!estop) {
    const float steer = std::clamp(steer_input, -1.0f, 1.0f);
    target = steer * p.max_steer_angle_rad;
  }

  const float tau = std::max(p.steer_tau_s, 1e-3f);
  const float next = prev_angle + (target - prev_angle) * static_cast<float>(dt_s / tau);

  return std::clamp(next, -p.max_steer_angle_rad, p.max_steer_angle_rad);
}

} // namespace Model
