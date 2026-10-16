#include "swc/vehicledynamics_swc.h"
#include "rte/rte.h"
#include "model/vehicledynamics_model.h"
#include "bsw/diag.h"

namespace Swc::VehicleDynamics {

static Model::VehicleParams g_params{};

void Init() {}
const char* Version() { return "VehicleDynamicsSWC-v0.0.2"; }

static Model::VehicleState ToModel(const Rte::VehicleState& s)
{
    Model::VehicleState m;
    m.t = s.t;
    m.x = s.x;
    m.y = s.y;
    m.yaw = s.yaw;
    m.v = s.v;
    m.yaw_rate = s.yaw_rate;
    m.wheel_omega = s.wheel_omega;
    return m;
}

static Rte::VehicleState FromModel(const Model::VehicleState& m)
{
    Rte::VehicleState s;
    s.t = m.t;
    s.x = m.x;
    s.y = m.y;
    s.yaw = m.yaw;
    s.v = m.v;
    s.yaw_rate = m.yaw_rate;
    s.wheel_omega = m.wheel_omega;
    return s;
}

void Step10ms(double dt_s)
{
    const auto st = Rte::Rte_Read_VehicleState();
    const auto cmd = Rte::Rte_Read_ActuatorCmd();
    const auto sf = Rte::Rte_Read_Safety();

    const bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

    const auto next = Model::StepVehicle(
        ToModel(st), static_cast<float>(dt_s),
        cmd.drive_accel_cmd, cmd.brake_decel_cmd, cmd.steer_angle_cmd,
        estop, g_params);

    Rte::Rte_Write_VehicleState(FromModel(next));
    Bsw::Diag::ReportAlive(Bsw::Diag::Component::VehicleDynamics);
}

} // namespace Swc::VehicleDynamics
