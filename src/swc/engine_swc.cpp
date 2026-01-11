#include "swc/engine_swc.h"
#include "rte/rte.h"
#include <algorithm>

namespace Swc::Engine {

struct Params {
    float max_accel_mps2 = 2.0f; // v1 default
};

static Params g_params{};

void Init() {}

const char* Version() { return "EngineSWC-v0.0.1"; }

void Main10ms(double /*dt_s*/)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    if (sf.estop || sf.system_state == Rte::SystemState::EStop) {
        cmd.drive_accel_cmd = 0.0f;
    } else {
        const float throttle = std::clamp(in.throttle, 0.0f, 1.0f);
        cmd.drive_accel_cmd = throttle * g_params.max_accel_mps2;
    }

    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Engine
