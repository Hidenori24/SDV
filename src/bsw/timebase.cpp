#include "bsw/timebase.h"
#include <cstdint>
#include <cmath>

namespace Bsw::TimeBase {

void Scheduler::AddTask10ms(TaskFn fn)  { tasks10ms_.push_back(std::move(fn)); }
void Scheduler::AddTask20ms(TaskFn fn)  { tasks20ms_.push_back(std::move(fn)); }
void Scheduler::AddTask100ms(TaskFn fn) { tasks100ms_.push_back(std::move(fn)); }

void Scheduler::RunForSeconds(double seconds)
{
    // v1: Deterministic single-thread fixed-step scheduler (no real-time sleep)
    const int64_t steps10ms = static_cast<int64_t>(std::round(seconds / 0.010));
    for (int64_t i = 0; i < steps10ms; ++i) {
        for (auto& t : tasks10ms_) t();

        if ((i % 2) == 0) { // 20ms
            for (auto& t : tasks20ms_) t();
        }
        if ((i % 10) == 0) { // 100ms
            for (auto& t : tasks100ms_) t();
        }
    }
}

} // namespace Bsw::TimeBase
