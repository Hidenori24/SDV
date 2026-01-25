#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "mock_rte.h"
#include "rte/rte.h"
#include "model/engine_model.h"

using Catch::Matchers::WithinAbs;

// ============================================================
// Test Version of Engine SWC (using Mock RTE)
// ============================================================
namespace Swc::Engine::Test {

static Model::EngineParams g_params{};

void Init() {}

void Main10ms_MockRTE(double /*dt_s*/)
{
    // Use Mock RTE instead of real RTE
    const auto in = Mock_Rte_Read_DriverInput();
    const auto sf = Mock_Rte_Read_Safety();

    auto cmd = Mock_Rte_Read_ActuatorCmd();

    bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

    cmd.drive_accel_cmd = Model::ComputeDriveAccel(in.throttle, estop, g_params);

    Mock_Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Engine::Test

// ============================================================
// Test Setup/Teardown
// ============================================================

struct EngineSwcTestFixture {
    EngineSwcTestFixture() {
        Mock_Rte_Reset();
        Swc::Engine::Test::Init();
    }

    ~EngineSwcTestFixture() {
        Mock_Rte_Reset();
    }
};

// ============================================================
// Test Cases
// ============================================================

TEST_CASE("EngineSWC: throttle 0.5 produces accel 1.0", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = 0.5, estop = false
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: drive_accel_cmd should be 0.5 * 2.0 = 1.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.0f, 0.01f));
}

TEST_CASE("EngineSWC: estop=true forces zero accel", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = 1.0 (max), but estop = true
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.0f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = true;  // E-Stop flag
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: estop should override throttle, drive_accel_cmd = 0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}

TEST_CASE("EngineSWC: throttle > 1.0 is clamped to max", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = 1.5 (exceeds range), estop = false
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.5f;  // > 1.0
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: throttle should be clamped to 1.0, so accel = 1.0 * 2.0 = 2.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(2.0f, 0.01f));
}

TEST_CASE("EngineSWC: system_state=EStop forces zero accel", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = 0.5, but system_state = EStop
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::EStop;  // System-level E-Stop

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: system_state EStop should also override, drive_accel_cmd = 0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}

TEST_CASE("EngineSWC: throttle 0.0 produces zero accel", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = 0.0 (no input)
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
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: no throttle should produce zero accel
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}

TEST_CASE("EngineSWC: negative throttle is clamped to zero", "[engine_swc]") {
    EngineSwcTestFixture fixture;

    // Setup: throttle = -0.5 (invalid negative)
    Rte::DriverInput driver_in{};
    driver_in.throttle = -0.5f;  // < 0
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute
    Swc::Engine::Test::Main10ms_MockRTE(0.01);

    // Verify: negative throttle should be clamped to 0.0, so accel = 0.0
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}
