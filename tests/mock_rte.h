

#ifndef MOCK_RTE_H
#define MOCK_RTE_H

#include "rte.h"

// ============================================================
// Mock RTE Global Signal Buffers
// ============================================================
static Rte::DriverInput g_driver_input{};
static Rte::Safety g_safety{};
static Rte::ActuatorCmd g_actuator_cmd{};

// ============================================================
// Mock RTE Read Functions (called by SWC)
// ============================================================

inline Rte::DriverInput Mock_Rte_Read_DriverInput()
{
    return g_driver_input;
}

inline Rte::Safety Mock_Rte_Read_Safety()
{
    return g_safety;
}

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

// ============================================================
// Mock RTE Helper Functions (called by Test)
// ============================================================

inline void Mock_Rte_SetDriverInput(const Rte::DriverInput& in)
{
    g_driver_input = in;
}

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