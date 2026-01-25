#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "mock_rte.h"
#include "rte/rte.h"
#include "model/brake_model.h"

using Catch::Matchers::WithinAbs;

// ============================================================
// Test Version of Brake SWC (using Mock RTE)
// ============================================================
namespace Swc::Brake::Test {

static Model::BrakeParams g_params{};

void Init() {}

void Main10ms_MockRTE(double /*dt_s*/)
{
    // Use Mock RTE instead of real RTE
    const auto in = Mock_Rte_Read_DriverInput();
    const auto sf = Mock_Rte_Read_Safety();

    auto cmd = Mock_Rte_Read_ActuatorCmd();

    bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

    cmd.brake_decel_cmd = Model::ComputeBrakeDecel(in.brake, estop, g_params);

    Mock_Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Brake::Test

// ============================================================
// Test Setup/Teardown
// ============================================================

struct BrakeSwcTestFixture {
    BrakeSwcTestFixture() {
        Mock_Rte_Reset();
        Swc::Brake::Test::Init();
    }

    ~BrakeSwcTestFixture() {
        Mock_Rte_Reset();
    }
};

// ============================================================
// Test Cases
// ============================================================

TEST_CASE("BrakeSWC: brake=0.5 produces decel 2.0", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = 0.5, estop = false
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 0.5f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: brake_decel_cmd should be 0.5 * 4.0 = 2.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(2.0f, 0.01f));
}

TEST_CASE("BrakeSWC: estop=true forces max deceleration", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = 0.0, but estop = true
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 0.0f;  // No brake input
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = true;  // E-Stop flag
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: estop should force max deceleration = 4.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeSWC: brake > 1.0 is clamped to max", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = 1.5 (exceeds range), estop = false
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 1.5f;  // > 1.0
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: brake should be clamped to 1.0, so decel = 1.0 * 4.0 = 4.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeSWC: system_state=EStop forces max deceleration", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = 0.5, but system_state = EStop
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 0.5f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::EStop;  // System-level E-Stop

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: system_state EStop should force max deceleration = 4.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeSWC: brake=0 produces zero deceleration", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = 0.0 (no input)
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: no brake should produce zero deceleration
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.brake_decel_cmd == 0.0f);
}

TEST_CASE("BrakeSWC: negative brake is clamped to zero", "[brake_swc]") {
    BrakeSwcTestFixture fixture;

    // Setup: brake = -0.5 (invalid negative)
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = -0.5f;  // < 0
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Brake::Test::Main10ms_MockRTE(0.01);

    // Verify: negative brake should be clamped to 0.0, so decel = 0.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.brake_decel_cmd == 0.0f);
}
