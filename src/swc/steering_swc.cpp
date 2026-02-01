#include "swc/steering_swc.h"
#include "model/steering_model.h"
#include "rte/rte.h"

namespace Swc::Steering {

static Model::SteeringParams g_params{};
static float g_steer_angle = 0.0f;

void Init() { g_steer_angle = 0.0f; }
const char* Version() { return "SteeringSWC-v0.0.1"; }

void Main10ms(double dt_s)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    const bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
    g_steer_angle = Model::StepSteerAngle(
        in.steer,
        estop,
        g_steer_angle,
        static_cast<float>(dt_s),
        g_params);

    cmd.steer_angle_cmd = g_steer_angle;
    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Steering
