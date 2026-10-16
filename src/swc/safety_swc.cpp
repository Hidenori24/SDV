#include "swc/safety_swc.h"
#include "rte/rte.h"
#include "bsw/diag.h"
#include "model/safety_logic.h"
#include <cmath>

namespace Swc::Safety {

static Model::SafetyParams g_params{};
static Model::HeartbeatMonitor g_monitors[static_cast<int>(Bsw::Diag::Component::Count)];
// A watchdog-triggered EStop is latched until Init() (like a real E-Stop that needs a reset).
static bool g_latched_estop = false;

void Init()
{
    for (auto& m : g_monitors) m = {};
    g_latched_estop = false;
}
const char* Version() { return "SafetySupervisorSWC-v0.1.0"; }

static bool alive(Bsw::Diag::Component c, uint32_t dt_ms)
{
    return Model::UpdateHeartbeat(g_monitors[static_cast<int>(c)], Bsw::Diag::GetAliveCount(c), dt_ms, g_params);
}

void Main100ms(double dt_s)
{
    using Bsw::Diag::Component;
    const auto dt_ms = static_cast<uint32_t>(std::lround(dt_s * 1000.0));

    Model::HeartbeatStatus hb;
    hb.engine_ok = alive(Component::Engine, dt_ms);
    hb.brake_ok = alive(Component::Brake, dt_ms);
    hb.steering_ok = alive(Component::Steering, dt_ms);
    hb.vehicledynamics_ok = alive(Component::VehicleDynamics, dt_ms);

    const Model::SafetyState wd_state = Model::ComputeSafetyState(hb);
    if (wd_state == Model::SafetyState::EStop) g_latched_estop = true;

    // sf.estop is the external E-Stop request (UI / hardware); the watchdog latch is OR-ed in.
    auto sf = Rte::Rte_Read_Safety();
    sf.estop = sf.estop || g_latched_estop;
    if (sf.estop) {
        sf.system_state = Rte::SystemState::EStop;
    } else {
        sf.system_state = static_cast<Rte::SystemState>(wd_state); // Normal or Degraded
    }
    Rte::Rte_Write_Safety(sf);

    Bsw::Diag::BumpHeartbeat();
}

} // namespace Swc::Safety
