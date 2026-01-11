#include "rte/rte.h"

namespace {
    Rte::DriverInput g_driver_input{};
    Rte::ActuatorCmd g_actuator_cmd{};
    Rte::VehicleState g_vehicle_state{};
    Rte::Safety g_safety{};
}

namespace Rte {

void InitDefaults()
{
    g_driver_input = DriverInput{};
    g_actuator_cmd = ActuatorCmd{};
    g_vehicle_state = VehicleState{};
    g_safety = Safety{};
}

DriverInput Rte_Read_DriverInput() { return g_driver_input; }
void Rte_Write_DriverInput(const DriverInput& v) { g_driver_input = v; }

ActuatorCmd Rte_Read_ActuatorCmd() { return g_actuator_cmd; }
void Rte_Write_ActuatorCmd(const ActuatorCmd& v) { g_actuator_cmd = v; }

VehicleState Rte_Read_VehicleState() { return g_vehicle_state; }
void Rte_Write_VehicleState(const VehicleState& v) { g_vehicle_state = v; }

Safety Rte_Read_Safety() { return g_safety; }
void Rte_Write_Safety(const Safety& v) { g_safety = v; }

} // namespace Rte
