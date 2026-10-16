#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <cstring>
#include <string>
#include <vector>

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

// Fault injection: "--stall <swc>:<t_s>" makes the SWC stop running after t_s (simulated time),
// so the Safety watchdog can be exercised. swc = engine|brake|steering|vehicledynamics.
struct Stall {
    const char* name;
    double from_s = -1.0; // < 0: never
};

static bool stalled(const Stall& s, double t) { return s.from_s >= 0.0 && t >= s.from_s; }

int main(int argc, char** argv)
{
    Stall st_engine{"engine"}, st_brake{"brake"}, st_steering{"steering"}, st_vd{"vehicledynamics"};
    Stall* stalls[] = {&st_engine, &st_brake, &st_steering, &st_vd};
    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--stall") != 0 || i + 1 >= argc) {
            std::fprintf(stderr, "usage: %s [--stall <engine|brake|steering|vehicledynamics>:<t_s>]...\n", argv[0]);
            return 2;
        }
        const std::string arg = argv[++i];
        const auto colon = arg.find(':');
        bool ok = false;
        if (colon != std::string::npos) {
            for (auto* s : stalls) {
                if (arg.compare(0, colon, s->name) == 0 && std::strlen(s->name) == colon) {
                    s->from_s = std::atof(arg.c_str() + colon + 1);
                    ok = true;
                }
            }
        }
        if (!ok) {
            std::fprintf(stderr, "invalid --stall argument: %s\n", arg.c_str());
            return 2;
        }
    }

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
        const double t = Rte::Rte_Read_VehicleState().t;
        if (!stalled(st_engine, t)) Swc::Engine::Main10ms(dt10);
        if (!stalled(st_brake, t)) Swc::Brake::Main10ms(dt10);
        if (!stalled(st_steering, t)) Swc::Steering::Main10ms(dt10);
        if (!stalled(st_vd, t)) Swc::VehicleDynamics::Step10ms(dt10);

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
