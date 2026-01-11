#include "swc/driverinput_swc.h"
#include "rte/rte.h"
#include <algorithm>
#include <cmath>

namespace Swc::DriverInput {

static float clampf(float v, float lo, float hi) { return std::max(lo, std::min(v, hi)); }

void Init() {}

void Main20ms(double /*dt_s*/)
{
    // v1 demo: built-in scenario (no external UI yet)
    // 0-2s: accelerate, 2-5s: cruise, 5-7s: brake, 7-10s: stop
    auto st = Rte::Rte_Read_VehicleState();
    Rte::DriverInput in = Rte::Rte_Read_DriverInput();

    if (st.t < 2.0f) {
        in.throttle = 0.6f;
        in.brake = 0.0f;
        in.steer = 0.0f;
    } else if (st.t < 5.0f) {
        in.throttle = 0.2f;
        in.brake = 0.0f;
        in.steer = 0.2f; // slight turn in demo
    } else if (st.t < 7.0f) {
        in.throttle = 0.0f;
        in.brake = 0.6f;
        in.steer = 0.0f;
    } else {
        in.throttle = 0.0f;
        in.brake = 0.0f;
        in.steer = 0.0f;
    }

    in.throttle = clampf(in.throttle, 0.0f, 1.0f);
    in.brake    = clampf(in.brake,    0.0f, 1.0f);
    in.steer    = clampf(in.steer,   -1.0f, 1.0f);

    Rte::Rte_Write_DriverInput(in);
}

} // namespace Swc::DriverInput
