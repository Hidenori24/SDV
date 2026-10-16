#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include <cmath>
#include "bsw/diag.h"
#include "rte/rte.h"
#include "swc/vehicledynamics_swc.h"

using Catch::Matchers::WithinAbs;

// These tests exercise the real VehicleDynamics SWC through the real RTE.
namespace {
struct VehicleSwcFixture {
    VehicleSwcFixture() {
        Rte::InitDefaults();
        Bsw::Diag::Init();
        Swc::VehicleDynamics::Init();
    }
    static void SetCmd(float drive, float brake, float steer) {
        Rte::ActuatorCmd c{};
        c.drive_accel_cmd = drive;
        c.brake_decel_cmd = brake;
        c.steer_angle_cmd = steer;
        Rte::Rte_Write_ActuatorCmd(c);
    }
    static void SetEstop(bool on) {
        Rte::Safety sf{};
        sf.estop = on;
        sf.system_state = on ? Rte::SystemState::EStop : Rte::SystemState::Normal;
        Rte::Rte_Write_Safety(sf);
    }
    static Rte::VehicleState Run(int steps) {
        for (int i = 0; i < steps; ++i) Swc::VehicleDynamics::Step10ms(0.01);
        return Rte::Rte_Read_VehicleState();
    }
};
}  // namespace

TEST_CASE("VehicleSWC: acceleration raises speed and wheel speed consistently", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(2.0f, 0.0f, 0.0f);
    auto st = VehicleSwcFixture::Run(100);
    REQUIRE(st.v > 1.0f);
    REQUIRE_THAT(st.wheel_omega, WithinAbs(st.v / 0.03f, 1e-3f));
    REQUIRE_THAT(st.t, WithinAbs(1.0f, 1e-4f));
}

TEST_CASE("VehicleSWC: speed is capped and never negative", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(10.0f, 0.0f, 0.0f);
    REQUIRE_THAT(VehicleSwcFixture::Run(1000).v, WithinAbs(3.0f, 1e-5f));
    VehicleSwcFixture::SetCmd(0.0f, 10.0f, 0.0f);
    REQUIRE(VehicleSwcFixture::Run(500).v >= 0.0f);
    REQUIRE(Rte::Rte_Read_VehicleState().v == 0.0f);
}

TEST_CASE("VehicleSWC: braking slows the car", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(2.0f, 0.0f, 0.0f);
    const float cruising = VehicleSwcFixture::Run(200).v;
    VehicleSwcFixture::SetCmd(0.0f, 2.0f, 0.0f);
    REQUIRE(VehicleSwcFixture::Run(50).v < cruising);
}

TEST_CASE("VehicleSWC: positive steer turns left, negative turns right", "[vehicledynamics_swc]") {
    {
        VehicleSwcFixture f;
        VehicleSwcFixture::SetCmd(1.0f, 0.0f, 0.2f);
        auto st = VehicleSwcFixture::Run(200);
        REQUIRE(st.yaw > 0.0f);
        REQUIRE(st.yaw_rate > 0.0f);
        REQUIRE(st.y > 0.0f);
    }
    {
        VehicleSwcFixture f;
        VehicleSwcFixture::SetCmd(1.0f, 0.0f, -0.2f);
        auto st = VehicleSwcFixture::Run(200);
        REQUIRE(st.yaw < 0.0f);
        REQUIRE(st.y < 0.0f);
    }
}

TEST_CASE("VehicleSWC: no steering drives straight along +x", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(1.0f, 0.0f, 0.0f);
    auto st = VehicleSwcFixture::Run(200);
    REQUIRE_THAT(st.yaw, WithinAbs(0.0f, 1e-6f));
    REQUIRE_THAT(st.y, WithinAbs(0.0f, 1e-6f));
    REQUIRE(st.x > 0.5f);
}

TEST_CASE("VehicleSWC: a stationary car does not turn even with steering", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(0.0f, 0.0f, 0.4f);
    auto st = VehicleSwcFixture::Run(50);
    REQUIRE_THAT(st.yaw, WithinAbs(0.0f, 1e-9f));
    REQUIRE_THAT(st.x, WithinAbs(0.0f, 1e-9f));
}

TEST_CASE("VehicleSWC: estop brings the car to a stop even with a stale drive command", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(2.0f, 0.0f, 0.0f);
    REQUIRE(VehicleSwcFixture::Run(300).v > 1.0f);
    VehicleSwcFixture::SetEstop(true);
    REQUIRE(VehicleSwcFixture::Run(200).v == 0.0f);
}

TEST_CASE("VehicleSWC: reports alive every step", "[vehicledynamics_swc]") {
    VehicleSwcFixture f;
    VehicleSwcFixture::SetCmd(0.0f, 0.0f, 0.0f);
    VehicleSwcFixture::Run(7);
    REQUIRE(Bsw::Diag::GetAliveCount(Bsw::Diag::Component::VehicleDynamics) == 7);
}
TEST_CASE("VehicleSWC: full lock at top speed is limited by tyre grip, at low speed it is not", "[vehicledynamics_swc]") {
    {
        VehicleSwcFixture f;
        VehicleSwcFixture::SetCmd(10.0f, 0.0f, 0.4f);
        auto st = VehicleSwcFixture::Run(1000);  // reaches the 3 m/s top speed
        REQUIRE_THAT(st.v, WithinAbs(3.0f, 1e-4f));
        REQUIRE(std::fabs(st.v * st.yaw_rate) <= 0.6f * 9.81f + 1e-2f);
        REQUIRE(st.yaw_rate < 2.1f);  // without the grip limit full lock would be 6.3 rad/s
    }
    {
        VehicleSwcFixture f;
        VehicleSwcFixture::SetCmd(0.0f, 0.0f, 0.4f);
        Rte::VehicleState s{};
        s.v = 1.0f;
        Rte::Rte_Write_VehicleState(s);
        auto st = VehicleSwcFixture::Run(1);
        REQUIRE(st.yaw_rate > 1.8f);  // 1 m/s: (v/L)*tan(0.4) ~ 2.1 rad/s, unaffected
    }
}
