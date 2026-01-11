#pragma once
#include <cstdint>

namespace Bsw::Diag {

void Init();
void Tick10ms();
void Tick100ms();

// v1 skeleton: you can expand to per-component heartbeat tracking later.
uint64_t GetHeartbeat();
void BumpHeartbeat();

} // namespace Bsw::Diag
