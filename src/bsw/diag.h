#pragma once
#include <cstdint>

namespace Bsw::Diag {

/// SWCs supervised by the Safety watchdog.
enum class Component : uint8_t {
    Engine = 0,
    Brake,
    Steering,
    VehicleDynamics,
    Count
};

void Init();
void Tick10ms();
void Tick100ms();

/// Alive supervision: each supervised SWC calls ReportAlive() every cycle.
void ReportAlive(Component c);
uint64_t GetAliveCount(Component c);

/// System heartbeat (bumped by the Safety SWC every cycle).
uint64_t GetHeartbeat();
void BumpHeartbeat();

} // namespace Bsw::Diag
