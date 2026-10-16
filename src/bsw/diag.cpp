#include "bsw/diag.h"

namespace {
    uint64_t g_hb = 0;
    uint64_t g_alive[static_cast<int>(Bsw::Diag::Component::Count)] = {};
}

namespace Bsw::Diag {

void Init()
{
    g_hb = 0;
    for (auto& a : g_alive) a = 0;
}
void Tick10ms() { /* reserved */ }
void Tick100ms() { /* reserved */ }

void ReportAlive(Component c) { ++g_alive[static_cast<int>(c)]; }
uint64_t GetAliveCount(Component c) { return g_alive[static_cast<int>(c)]; }

uint64_t GetHeartbeat() { return g_hb; }
void BumpHeartbeat() { ++g_hb; }

} // namespace Bsw::Diag
