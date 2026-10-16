#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "rte/rte.h"
#include "swc/steering_swc.h"

using Catch::Matchers::WithinAbs;

// These tests exercise the real Steering SWC through the real RTE.
namespace {
struct SteeringSwcFixture {
    SteeringSwcFixture() {
        Rte::InitDefaults();
        Swc::Steering::Init();
    }
    static void SetInput(float steer) {
        Rte::DriverInput in{};
        in.steer = steer;
        Rte::Rte_Write_DriverInput(in);
    }
    static void SetEstop(bool estop) {
        Rte::Safety sf{};
        sf.estop = estop;
        sf.system_state = estop ? Rte::SystemState::EStop : Rte::SystemState::Normal;
        Rte::Rte_Write_Safety(sf);
    }
    static float Run(int steps) {
        for (int i = 0; i < steps; ++i) Swc::Steering::Main10ms(0.01);
        return Rte::Rte_Read_ActuatorCmd().steer_angle_cmd;
    }
};
} // namespace

TEST_CASE("SteeringSWC: steer=1.0 ramps up gradually", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(1.0f);

    const float a1 = SteeringSwcFixture::Run(1);
    REQUIRE(a1 > 0.0f);
    REQUIRE(a1 < 0.4f * 0.5f); // lag: one step is far from the target

    const float a2 = SteeringSwcFixture::Run(1);
    REQUIRE(a2 > a1);

    const float a_end = SteeringSwcFixture::Run(200);
    REQUIRE_THAT(a_end, WithinAbs(0.4f, 1e-3f));
}

TEST_CASE("SteeringSWC: steer=-1.0 steers the opposite way", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(-1.0f);
    REQUIRE(SteeringSwcFixture::Run(1) < 0.0f);
    REQUIRE_THAT(SteeringSwcFixture::Run(200), WithinAbs(-0.4f, 1e-3f));
}

TEST_CASE("SteeringSWC: out-of-range steer is saturated", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(5.0f);
    REQUIRE_THAT(SteeringSwcFixture::Run(300), WithinAbs(0.4f, 1e-3f));
}

TEST_CASE("SteeringSWC: estop forces the angle back to zero", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(1.0f);
    const float steered = SteeringSwcFixture::Run(100);
    REQUIRE(steered > 0.3f);

    SteeringSwcFixture::SetEstop(true);
    float prev = steered;
    for (int i = 0; i < 100; ++i) {
        const float a = SteeringSwcFixture::Run(1);
        REQUIRE(a <= prev);
        prev = a;
    }
    REQUIRE_THAT(prev, WithinAbs(0.0f, 1e-3f));
}

TEST_CASE("SteeringSWC: system_state EStop alone also forces zero target", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(1.0f);
    Rte::Safety sf{};
    sf.estop = false;
    sf.system_state = Rte::SystemState::EStop;
    Rte::Rte_Write_Safety(sf);
    REQUIRE_THAT(SteeringSwcFixture::Run(100), WithinAbs(0.0f, 1e-6f));
}

TEST_CASE("SteeringSWC: Init resets the internal state", "[steering_swc]") {
    SteeringSwcFixture f;
    SteeringSwcFixture::SetInput(1.0f);
    SteeringSwcFixture::Run(50);
    Swc::Steering::Init();
    SteeringSwcFixture::SetInput(0.0f);
    REQUIRE_THAT(SteeringSwcFixture::Run(1), WithinAbs(0.0f, 1e-6f));
}

TEST_CASE("SteeringSWC: preserves other actuator commands", "[steering_swc]") {
    SteeringSwcFixture f;
    Rte::ActuatorCmd cmd{};
    cmd.drive_accel_cmd = 1.5f;
    cmd.brake_decel_cmd = 2.5f;
    Rte::Rte_Write_ActuatorCmd(cmd);
    SteeringSwcFixture::SetInput(0.5f);
    SteeringSwcFixture::Run(1);
    const auto out = Rte::Rte_Read_ActuatorCmd();
    REQUIRE(out.drive_accel_cmd == 1.5f);
    REQUIRE(out.brake_decel_cmd == 2.5f);
}
