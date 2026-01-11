#include "swc/brake_swc.h"
#include "rte/rte.h"
#include <algorithm>

namespace Swc::Brake {

struct Params {
    float max_decel_mps2 = 4.0f; // v1 default
};

static Params g_params{};

void Init() {}
const char* Version() { return "BrakeSWC-v0.0.1"; }

void Main10ms(double /*dt_s*/)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    if (sf.estop || sf.system_state == Rte::SystemState::EStop) {
        cmd.brake_decel_cmd = g_params.max_decel_mps2; // force braking
    } else {
        const float brake = std::clamp(in.brake, 0.0f, 1.0f);
        cmd.brake_decel_cmd = brake * g_params.max_decel_mps2;
    }

    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Brake
