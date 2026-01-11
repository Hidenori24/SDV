#pragma once
#include <functional>
#include <vector>

namespace Bsw::TimeBase {

using TaskFn = std::function<void()>;

class Scheduler {
public:
    void AddTask10ms(TaskFn fn);
    void AddTask20ms(TaskFn fn);
    void AddTask100ms(TaskFn fn);

    void RunForSeconds(double seconds);

private:
    std::vector<TaskFn> tasks10ms_;
    std::vector<TaskFn> tasks20ms_;
    std::vector<TaskFn> tasks100ms_;
};

} // namespace Bsw::TimeBase
