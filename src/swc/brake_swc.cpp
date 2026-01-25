#include "swc/brake_swc.h"
#include "rte/rte.h"
#include "model/brake_model.h"

namespace Swc::Brake {

static Model::BrakeParams g_params{};

void Init() {}
const char* Version() { return "BrakeSWC-v0.0.1"; }

void Main10ms(double /*dt_s*/)
{
    const auto in = Rte::Rte_Read_DriverInput();
    const auto sf = Rte::Rte_Read_Safety();

    auto cmd = Rte::Rte_Read_ActuatorCmd();

    bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
    cmd.brake_decel_cmd = Model::ComputeBrakeDecel(in.brake, estop, g_params);

    Rte::Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Brake
