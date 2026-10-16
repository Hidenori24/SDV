#pragma once
#include <algorithm>
#include <cmath>

#include "model/brake_model.h"
#include "model/engine_model.h"
#include "model/steering_model.h"
#include "model/vehicledynamics_model.h"

namespace Model {

/**
 * @brief The car itself: dimensions and performance (from the data sheet or measured).
 *
 * Used by the plant (simulator) and by the ECU software on the real car.
 */
struct VehicleSpec {
  float wheelbase_m = 0.20f;
  float wheel_radius_m = 0.03f;
  float max_steer_angle_rad = 0.40f;   ///< mechanical limit of the road wheels
  float steer_time_constant_s = 0.15f; ///< steering servo lag (first order)
  float top_speed_mps = 3.0f;
  float max_accel_mps2 = 2.0f;
  float max_decel_mps2 = 4.0f;
  float estop_decel_mps2 = 6.0f;       ///< emergency stop ability
};

/**
 * @brief The world: values nature decides. Used by the plant only (on the real car nature supplies them).
 */
struct Physics {
  float gravity_mps2 = 9.81f;
  float tire_friction_mu = 0.6f;  ///< depends on the surface (floor, carpet, asphalt)
  float drag_coeff = 0.15f;       ///< simple linear running resistance
};

/**
 * @brief Values computed from spec + physics (read-only, never set by hand).
 */
struct Derived {
  float min_turn_radius_m = 0.0f;       ///< L / tan(max steer)
  float max_lateral_accel_mps2 = 0.0f;  ///< mu * g
  float critical_speed_mps = 0.0f;      ///< above this speed full lock is limited by grip
};

inline Derived Derive(const VehicleSpec& spec, const Physics& phys)
{
  Derived d;
  const float L = std::max(spec.wheelbase_m, 1e-3f);
  d.min_turn_radius_m = L / std::tan(spec.max_steer_angle_rad);
  d.max_lateral_accel_mps2 = phys.tire_friction_mu * phys.gravity_mps2;
  d.critical_speed_mps = std::sqrt(d.max_lateral_accel_mps2 * d.min_turn_radius_m);
  return d;
}

/**
 * @brief Largest steering angle that takes effect at speed v: min(max steer, atan(L * mu * g / v^2))
 */
inline float MaxSteerAtSpeed(float v, const VehicleSpec& spec, const Physics& phys)
{
  if (v <= 0.0f) return spec.max_steer_angle_rad;
  const float L = std::max(spec.wheelbase_m, 1e-3f);
  const float a_lat_max = std::max(phys.tire_friction_mu * phys.gravity_mps2, 1e-3f);
  return std::min(spec.max_steer_angle_rad, std::atan(L * a_lat_max / (v * v)));
}

/**
 * @brief Yaw rate (rad/s) the plant produces at speed v with the steering at full lock
 */
inline float YawRateAtFullLock(float v, const VehicleSpec& spec, const Physics& phys)
{
  const float L = std::max(spec.wheelbase_m, 1e-3f);
  return (v / L) * std::tan(MaxSteerAtSpeed(v, spec, phys));
}

// ---- spec + physics -> the parameter structs of the individual models ----

inline VehicleParams MakeVehicleParams(const VehicleSpec& spec, const Physics& phys)
{
  VehicleParams p;
  p.wheel_radius_m = spec.wheel_radius_m;
  p.wheelbase_m = spec.wheelbase_m;
  p.linear_drag = phys.drag_coeff;
  p.max_speed_mps = spec.top_speed_mps;
  p.estop_decel_mps2 = spec.estop_decel_mps2;
  p.tire_friction_mu = phys.tire_friction_mu;
  p.gravity_mps2 = phys.gravity_mps2;
  return p;
}

inline EngineParams MakeEngineParams(const VehicleSpec& spec)
{
  EngineParams p;
  p.max_accel_mps2 = spec.max_accel_mps2;
  return p;
}

inline BrakeParams MakeBrakeParams(const VehicleSpec& spec, float estop_max_decel_mps2)
{
  BrakeParams p;
  p.max_decel_mps2 = spec.max_decel_mps2;
  p.estop_max_decel_mps2 = estop_max_decel_mps2;
  return p;
}

inline SteeringParams MakeSteeringParams(const VehicleSpec& spec)
{
  SteeringParams p;
  p.max_steer_angle_rad = spec.max_steer_angle_rad;
  p.time_constant_s = spec.steer_time_constant_s;
  return p;
}

} // namespace Model
