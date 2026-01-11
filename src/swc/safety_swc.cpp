#include "swc/safety_swc.h"
#include "rte/rte.h"
#include "bsw/diag.h"

namespace Swc::Safety {

void Init() {}
const char* Version() { return "SafetySupervisorSWC-v0.0.1"; }

void Main100ms(double /*dt_s*/)
{
    // v1 skeleton: no per-component heartbeat yet.
    // This SWC exists so architecture is in place. Extend here later.
    // Example: if heartbeat stops progressing -> EStop.
    auto sf = Rte::Rte_Read_Safety();
    sf.system_state = sf.estop ? Rte::SystemState::EStop : Rte::SystemState::Normal;
    Rte::Rte_Write_Safety(sf);

    // bump system heartbeat for now
    Bsw::Diag::BumpHeartbeat();
}

} // namespace Swc::Safety
