#pragma once
#include <algorithm>

namespace Model {

struct EngineParams {
  float max_accel_mps2 = 2.0f;
};

inline float ComputeDriveAccel(float throttle_0_1, bool estop, const EngineParams& p) {
  if (estop) return 0.0f;
  float th = std::clamp(throttle_0_1, 0.0f, 1.0f);
  return th * p.max_accel_mps2;
}

} // namespace Model
