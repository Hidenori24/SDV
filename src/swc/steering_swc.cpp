#include "swc/steering_swc.h"
#include "rte/rte.h"
#include <algorithm>
#include <cmath>

namespace Swc::Steering {

struct Params {
    float max_steer_angle_rad = 0.40f; // ~23 deg
    float steer_tau_s = 0.15f;         // first-order lag
};

static Params g_params{};
static float g_steer_angle = 0.0f;

void Init() { g_steer_angle = 0.0f; }
const char* Version() { return "SteeringSWC-v0.0.1"; }

void Main10ms(double dt_s)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    float target = 0.0f;
    if (!sf.estop && sf.system_state != Rte::SystemState::EStop) {
        const float steer = std::clamp(in.steer, -1.0f, 1.0f);
        target = steer * g_params.max_steer_angle_rad;
    }

    // First-order lag: d/dt x = (target - x) / tau
    const float tau = std::max(g_params.steer_tau_s, 1e-3f);
    g_steer_angle += static_cast<float>((target - g_steer_angle) * (dt_s / tau));

    cmd.steer_angle_cmd = std::clamp(g_steer_angle, -g_params.max_steer_angle_rad, g_params.max_steer_angle_rad);
    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Steering
