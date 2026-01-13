/**
 * @file mock_rte.h
 * @brief Mock RTE for SWC Unit Testing
 * 
 * This header provides a lightweight Mock RTE implementation for testing
 * Software Components (SWC) without requiring the full RTE infrastructure.
 * 
 * @section Usage
 * @code
 * // 1. Setup: Set input signals
 * Mock_Rte_SetDriverInput({.throttle = 0.5f, .brake = 0.0f, .steer = 0.0f});
 * Mock_Rte_SetSafety({.estop = false, .system_state = Rte::SystemState::Normal});
 * 
 * // 2. Execute: Call SWC under test
 * Swc::Engine::Main10ms(0.01);
 * 
 * // 3. Verify: Check output signals
 * auto cmd = Mock_Rte_Written_ActuatorCmd();
 * REQUIRE(cmd.drive_accel_cmd == Approx(1.0f));
 * @endcode
 */

#ifndef MOCK_RTE_H
#define MOCK_RTE_H

#include "../src/rte/rte.h"

// ============================================================
// Mock RTE Global Signal Buffers
// ============================================================
static Rte::DriverInput g_driver_input{};
static Rte::Safety g_safety{};
static Rte::ActuatorCmd g_actuator_cmd{};

// ============================================================
// Mock RTE Read Functions (called by SWC)
// ============================================================

/**
 * @brief Read DriverInput signal from Mock RTE
 * @return Current DriverInput state
 * @note Called by SWC to read driver inputs (throttle, brake, steer)
 */
inline Rte::DriverInput Mock_Rte_Read_DriverInput()
{
    return g_driver_input;
}

/**
 * @brief Read Safety signal from Mock RTE
 * @return Current Safety state
 * @note Called by SWC to read safety status (estop, system_state)
 */
inline Rte::Safety Mock_Rte_Read_Safety()
{
    return g_safety;
}
/**
 * @brief Write ActuatorCmd signal to Mock RTE
 * @param cmd ActuatorCmd to write
 * @note Called by SWC to write computed actuator commands
 */

/**
 * @brief Read ActuatorCmd signal from Mock RTE
 * @return Current ActuatorCmd state
 * @note Called by SWC to read actuator commands (initial value)
 */
inline Rte::ActuatorCmd Mock_Rte_Read_ActuatorCmd()
{
    return g_actuator_cmd;
}

// ============================================================
// Mock RTE Write Functions (called by SWC)
// ============================================================

inline void Mock_Rte_Write_ActuatorCmd(const Rte::ActuatorCmd& cmd)
{
    g_actuator_cmd = cmd;
}
/**
 * @brief Set DriverInput signal in Mock RTE
 * @param in DriverInput to set
 * @note Called by test to prepare input before SWC execution
 */
inline void Mock_Rte_SetDriverInput(const Rte::DriverInput& in)
{
    g_driver_input = in;
}

/**
 * @brief Set Safety signal in Mock RTE
 * @param sf Safety status to set
 * @note Called by test to prepare safety state before SWC execution
 */
// inline void Mock_Rte_SetSafety(const Rte::Safety& sf)
// {
//     g_safety = sf;
// }
/**
 * @brief Reset all Mock RTE signals to default values
 * @note Call this between test cases to ensure clean state
 * @code
 * TEST_CASE("Test case 1") {
 *     Mock_Rte_Reset();  // Clean slate
 *     // ... test code
 * }
 * @endcode
 */

/**
 * @brief Get ActuatorCmd written by SWC
 * @return ActuatorCmd written by last SWC execution
 * @note Called by test to verify SWC output after execution
 */
inline void Mock_Rte_SetSafety(const Rte::Safety& sf)
{
    g_safety = sf;
}

inline Rte::ActuatorCmd Mock_Rte_Written_ActuatorCmd()
{
    return g_actuator_cmd;
}

// ============================================================
// Mock RTE Reset (optional, useful for test setup)
// ============================================================

inline void Mock_Rte_Reset()
{
    g_driver_input = {};
    g_safety = {};
    g_actuator_cmd = {};
}

#endif // MOCK_RTE_H