#include "swc/steering_swc.h"
#include "rte/rte.h"
#include "model/steering_model.h"
#include "bsw/diag.h"

namespace Swc::Steering {

static Model::SteeringParams g_params{};
static float g_steer_angle = 0.0f;

void Init() { g_steer_angle = 0.0f; }
const char* Version() { return "SteeringSWC-v0.0.2"; }

void Main10ms(double dt_s)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();
    const bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

    const float target = Model::ComputeSteerTarget(in.steer, estop, g_params);
    g_steer_angle = Model::StepSteeringDynamics(g_steer_angle, target, static_cast<float>(dt_s), g_params);

    auto cmd = Rte::Rte_Read_ActuatorCmd();
    cmd.steer_angle_cmd = g_steer_angle;
    Rte::Rte_Write_ActuatorCmd(cmd);
    Bsw::Diag::ReportAlive(Bsw::Diag::Component::Steering);
}

} // namespace Swc::Steering
