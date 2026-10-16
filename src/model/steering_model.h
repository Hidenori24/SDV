#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

/**
 * @brief Steering Model Parameters
 */
struct SteeringParams {
    float max_steer_angle_rad = 0.40f; ///< Maximum steering angle (rad), ~23 deg
    float time_constant_s = 0.15f;     ///< First-order lag time constant (s)
};

/**
 * @brief Convert normalized driver steer input to target steering angle
 *
 * @param steer_norm Driver steer input, clamped to [-1, 1]
 * @param estop Emergency stop flag (true = target angle is 0)
 * @return Target steering angle in rad, saturated to +/- max_steer_angle_rad
 */
inline float ComputeSteerTarget(float steer_norm, bool estop, const SteeringParams& p)
{
    if (estop) {
        return 0.0f;
    }
    const float steer = std::clamp(steer_norm, -1.0f, 1.0f);
    return std::clamp(steer * p.max_steer_angle_rad, -p.max_steer_angle_rad, p.max_steer_angle_rad);
}

/**
 * @brief One step of first-order lag steering dynamics (pure function)
 *
 * new = current + (1 - exp(-dt/tau)) * (target - current)
 * The exact discretization is used, so the result does not overshoot for any dt.
 *
 * @param current_angle Current steering angle (rad)
 * @param target_angle Target steering angle (rad); saturated to +/- max_steer_angle_rad
 * @param dt_s Step time (s); dt <= 0 leaves the angle unchanged
 * @return Next steering angle (rad)
 */
inline float StepSteeringDynamics(float current_angle, float target_angle, float dt_s, const SteeringParams& p)
{
    const float target = std::clamp(target_angle, -p.max_steer_angle_rad, p.max_steer_angle_rad);
    if (dt_s <= 0.0f) {
        return current_angle;
    }
    const float tau = std::max(p.time_constant_s, 1e-3f);
    const float alpha = 1.0f - std::exp(-dt_s / tau);
    return current_angle + alpha * (target - current_angle);
}

} // namespace Model
