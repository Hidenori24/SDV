#pragma once
#include <cstdint>

namespace Rte {

enum class SystemState : uint8_t {
    Normal = 0,
    Degraded = 1,
    EStop = 2
};

struct DriverInput {
    float throttle = 0.0f; // 0..1
    float brake    = 0.0f; // 0..1
    float steer    = 0.0f; // -1..1
};

struct ActuatorCmd {
    float drive_accel_cmd = 0.0f; // m/s^2
    float brake_decel_cmd = 0.0f; // m/s^2 (positive -> decel)
    float steer_angle_cmd = 0.0f; // rad
};

struct VehicleState {
    float t = 0.0f;          // s
    float x = 0.0f;          // m
    float y = 0.0f;          // m
    float yaw = 0.0f;        // rad
    float v = 0.0f;          // m/s
    float yaw_rate = 0.0f;   // rad/s
    float wheel_omega = 0.0f;// rad/s
};

struct Safety {
    bool estop = false;
    SystemState system_state = SystemState::Normal;
};

void InitDefaults();

// Read / Write APIs (AUTOSAR-like style)
DriverInput Rte_Read_DriverInput();
void Rte_Write_DriverInput(const DriverInput& v);

ActuatorCmd Rte_Read_ActuatorCmd();
void Rte_Write_ActuatorCmd(const ActuatorCmd& v);

VehicleState Rte_Read_VehicleState();
void Rte_Write_VehicleState(const VehicleState& v);

Safety Rte_Read_Safety();
void Rte_Write_Safety(const Safety& v);

} // namespace Rte

// Convenience global wrapper (keeps docs terminology)
inline void Rte_InitDefaults() { Rte::InitDefaults(); }
