#include "swc/engine_swc.h"
#include "rte/rte.h"
#include <algorithm>

#include "model/engine_model.h"

namespace Swc::Engine {

// struct Params {
//     float max_accel_mps2 = Model::EngineParams{}.max_accel_mps2;
// };

static Model::EngineParams g_params{};

void Init() {}

const char* Version() { return "EngineSWC-v0.0.1"; }

void Main10ms(double /*dt_s*/)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

    cmd.drive_accel_cmd = Model::ComputeDriveAccel(in.throttle, estop, g_params);

    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Engine
