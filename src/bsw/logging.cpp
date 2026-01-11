#include "bsw/logging.h"
#include "rte/rte.h"
#include <cstdio>
#include <filesystem>

namespace {
    std::FILE* g_fp = nullptr;

    void write_header()
    {
        std::fprintf(g_fp,
            "t,throttle,brake,steer,"
            "drive_accel_cmd,brake_decel_cmd,steer_angle_cmd,"
            "x,y,yaw,v,yaw_rate,wheel_omega,"
            "estop,system_state\n");
    }
}

namespace Bsw::Logging {

void Init(const std::string& path)
{
    std::filesystem::create_directories(std::filesystem::path(path).parent_path());
    g_fp = std::fopen(path.c_str(), "w");
    if (!g_fp) {
        std::perror("Failed to open log file");
        std::abort();
    }
    write_header();
}

void Tick10ms()
{
    if (!g_fp) return;

    const auto in = Rte::Rte_Read_DriverInput();
    const auto cmd = Rte::Rte_Read_ActuatorCmd();
    const auto st = Rte::Rte_Read_VehicleState();
    const auto sf = Rte::Rte_Read_Safety();

    std::fprintf(g_fp,
        "%.3f,%.3f,%.3f,%.3f,"
        "%.3f,%.3f,%.6f,"
        "%.3f,%.3f,%.6f,%.3f,%.6f,%.3f,"
        "%d,%u\n",
        st.t, in.throttle, in.brake, in.steer,
        cmd.drive_accel_cmd, cmd.brake_decel_cmd, cmd.steer_angle_cmd,
        st.x, st.y, st.yaw, st.v, st.yaw_rate, st.wheel_omega,
        sf.estop ? 1 : 0, static_cast<unsigned>(sf.system_state)
    );
}

} // namespace Bsw::Logging
