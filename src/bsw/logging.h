#pragma once
#include <string>

namespace Bsw::Logging {

void Init(const std::string& path);
void Tick10ms();

} // namespace Bsw::Logging
