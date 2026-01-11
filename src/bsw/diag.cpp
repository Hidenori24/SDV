#include "bsw/diag.h"

namespace {
    uint64_t g_hb = 0;
}

namespace Bsw::Diag {

void Init() { g_hb = 0; }
void Tick10ms() { /* reserved */ }
void Tick100ms() { /* reserved */ }

uint64_t GetHeartbeat() { return g_hb; }
void BumpHeartbeat() { ++g_hb; }

} // namespace Bsw::Diag
