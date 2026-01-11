#include <cstdio>
#include <cstdlib>
#include <filesystem>

#include "bsw/timebase.h"
#include "bsw/logging.h"
#include "bsw/diag.h"
#include "rte/rte.h"

#include "swc/driverinput_swc.h"
#include "swc/engine_swc.h"
#include "swc/brake_swc.h"
#include "swc/steering_swc.h"
#include "swc/vehicledynamics_swc.h"
#include "swc/safety_swc.h"

static void ensure_logs_dir()
{
    std::filesystem::create_directories("logs");
}

int main()
{
    ensure_logs_dir();

    // Init RTE default values
    Rte_InitDefaults();

    // Init services
    Bsw::Diag::Init();
    Bsw::Logging::Init("logs/latest.csv");

    // Init SWCs (if needed)
    Swc::DriverInput::Init();
    Swc::Engine::Init();
    Swc::Brake::Init();
    Swc::Steering::Init();
    Swc::VehicleDynamics::Init();
    Swc::Safety::Init();

    // Fixed-step scheduler (v1 skeleton)
    //  - 10ms: control + plant + logging
    //  - 20ms: driver input
    //  - 100ms: safety + diag
    Bsw::TimeBase::Scheduler sched;

    constexpr double dt10 = 0.010;
    constexpr double dt20 = 0.020;

    sched.AddTask10ms([&]{
        Swc::Engine::Main10ms(dt10);
        Swc::Brake::Main10ms(dt10);
        Swc::Steering::Main10ms(dt10);
        Swc::VehicleDynamics::Step10ms(dt10);

        Bsw::Diag::Tick10ms();
        Bsw::Logging::Tick10ms();
    });

    sched.AddTask20ms([&]{
        Swc::DriverInput::Main20ms(dt20);
    });

    sched.AddTask100ms([&]{
        Swc::Safety::Main100ms(0.100);
        Bsw::Diag::Tick100ms();
    });

    // Run a short demo loop (10 seconds) so the repo "does something" out of the box.
    // Input is a simple built-in scenario for now; later replace with Com/UI.
    constexpr double sim_seconds = 10.0;
    sched.RunForSeconds(sim_seconds);

    std::printf("Done. Log written to logs/latest.csv\n");
    return 0;
}
