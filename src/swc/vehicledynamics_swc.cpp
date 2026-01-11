#include "swc/vehicledynamics_swc.h"
#include "rte/rte.h"
#include <algorithm>
#include <cmath>

namespace Swc::VehicleDynamics {

struct Params {
    float wheel_radius_m = 0.03f;  // 3cm (toy-scale placeholder)
    float wheelbase_m    = 0.20f;  // 20cm
    float linear_drag    = 0.15f;  // simple resist coefficient
    float max_speed_mps  = 3.0f;   // cap for v1
    float estop_decel_mps2 = 6.0f; // extra forced decel when estop
};

static Params g_params{};

void Init() {}
const char* Version() { return "VehicleDynamicsSWC-v0.0.1"; }

static float resist(float v)
{
    // Very simple: linear drag
    return g_params.linear_drag * v;
}

void Step10ms(double dt_s)
{
    auto st = Rte::Rte_Read_VehicleState();
    const auto cmd = Rte::Rte_Read_ActuatorCmd();
    const auto sf = Rte::Rte_Read_Safety();

    const float dt = static_cast<float>(dt_s);

    float accel = cmd.drive_accel_cmd - cmd.brake_decel_cmd - resist(st.v);
    if (sf.estop || sf.system_state == Rte::SystemState::EStop) {
        accel -= g_params.estop_decel_mps2;
    }

    st.v = st.v + accel * dt;
    st.v = std::clamp(st.v, 0.0f, g_params.max_speed_mps);

    // Bicycle model
    const float L = std::max(g_params.wheelbase_m, 1e-3f);
    st.yaw_rate = (st.v / L) * std::tan(cmd.steer_angle_cmd);
    st.yaw = st.yaw + st.yaw_rate * dt;

    st.x = st.x + st.v * std::cos(st.yaw) * dt;
    st.y = st.y + st.v * std::sin(st.yaw) * dt;

    const float r = std::max(g_params.wheel_radius_m, 1e-4f);
    st.wheel_omega = st.v / r; // rad/s (no gear ratio)

    st.t = st.t + dt;

    Rte::Rte_Write_VehicleState(st);
}

} // namespace Swc::VehicleDynamics
