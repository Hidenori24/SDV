#pragma once
#include <algorithm>

namespace Model {

/**
 * @brief Brake Model Parameters
 */
struct BrakeParams {
    float max_decel_mps2 = 4.0f;        ///< Maximum brake deceleration (m/s²)
    float estop_max_decel_mps2 = 4.0f;  ///< Emergency stop deceleration (m/s²)
};

/**
 * @brief Compute brake deceleration command
 * 
 * Pure function that converts brake pedal input (0..1) to deceleration command (m/s²).
 * In emergency stop mode, returns maximum deceleration regardless of brake input.
 * 
 * @param brake_0_1 Brake pedal input, normalized 0..1 (0=no brake, 1=full brake)
 * @param estop Emergency stop flag (true = force maximum deceleration)
 * @param p Brake parameters containing max_decel_mps2
 * @return Brake deceleration command in m/s² (positive value means deceleration)
 * 
 * @note This is a pure function with no side effects
 * @note brake_0_1 is clamped to [0.0, 1.0] range
 * @note When estop=true, always returns estop_max_decel_mps2
 * 
 * Example:
 * @code
 * BrakeParams p{.max_decel_mps2 = 4.0f};
 * float decel = ComputeBrakeDecel(0.5f, false, p);  // Returns 2.0 m/s²
 * float estop_decel = ComputeBrakeDecel(0.5f, true, p);  // Returns 4.0 m/s²
 * @endcode
 */
inline float ComputeBrakeDecel(float brake_0_1, bool estop, const BrakeParams& p)
{
    // Emergency stop: force maximum deceleration
    if (estop) {
        return p.estop_max_decel_mps2;
    }

    // Normal braking: clamp input and scale by max_decel
    float brake_clamped = std::clamp(brake_0_1, 0.0f, 1.0f);
    return brake_clamped * p.max_decel_mps2;
}

} // namespace Model
