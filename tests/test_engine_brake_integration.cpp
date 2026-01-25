#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "mock_rte.h"
#include "rte/rte.h"
#include "model/engine_model.h"
#include "model/brake_model.h"

using Catch::Matchers::WithinAbs;

// ============================================================
// Integration Test: Engine + Brake SWC
// ============================================================
namespace Swc::Integration::Test {

static Model::EngineParams engine_params{};
static Model::BrakeParams brake_params{};

void ExecuteEngineAndBrakeCycle(double dt_s)
{
    // Engine SWC execution
    {
        const auto in = Mock_Rte_Read_DriverInput();
        const auto sf = Mock_Rte_Read_Safety();

        auto cmd = Mock_Rte_Read_ActuatorCmd();

        bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
        cmd.drive_accel_cmd = Model::ComputeDriveAccel(in.throttle, estop, engine_params);

        Mock_Rte_Write_ActuatorCmd(cmd);
    }

    // Brake SWC execution (uses same RTE buffers)
    {
        const auto in = Mock_Rte_Read_DriverInput();
        const auto sf = Mock_Rte_Read_Safety();

        auto cmd = Mock_Rte_Read_ActuatorCmd();  // Reads what Engine wrote

        bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
        cmd.brake_decel_cmd = Model::ComputeBrakeDecel(in.brake, estop, brake_params);

        Mock_Rte_Write_ActuatorCmd(cmd);  // Updates cmd with brake_decel_cmd
    }
}

} // namespace Swc::Integration::Test

// ============================================================
// Test Fixture
// ============================================================

struct IntegrationTestFixture {
    IntegrationTestFixture() {
        Mock_Rte_Reset();
    }

    ~IntegrationTestFixture() {
        Mock_Rte_Reset();
    }
};

// ============================================================
// Integration Test Cases
// ============================================================

TEST_CASE("Integration: Normal acceleration with no brake", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: full throttle, no brake
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.0f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Engine should output max accel, Brake should output no decel
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(2.0f, 0.01f));
    REQUIRE(cmd.brake_decel_cmd == 0.0f);
}

TEST_CASE("Integration: Partial acceleration with partial brake", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: half throttle, half brake (realistic combined input)
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    driver_in.brake = 0.5f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Both should scale proportionally
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.0f, 0.01f));  // 0.5 * 2.0
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(2.0f, 0.01f));  // 0.5 * 4.0
}

TEST_CASE("Integration: E-Stop overrides both engine and brake", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: e-stop with arbitrary throttle/brake input
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.8f;
    driver_in.brake = 0.2f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = true;  // E-Stop flag
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Engine should zero out, Brake should maximize
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);  // Engine forced to zero by estop
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(4.0f, 0.01f));  // Brake forced to max
}

TEST_CASE("Integration: No throttle + no brake = coast", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: coasting (neither throttle nor brake)
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.0f;
    driver_in.brake = 0.0f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: No acceleration, no braking
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
    REQUIRE(cmd.brake_decel_cmd == 0.0f);
}

TEST_CASE("Integration: System state Degraded (normal operation)", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: degraded system with normal inputs
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.6f;
    driver_in.brake = 0.3f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Degraded;  // Degraded but not emergency stop

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Both should operate normally (Degraded != EStop)
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.2f, 0.01f));  // 0.6 * 2.0
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(1.2f, 0.01f));  // 0.3 * 4.0
}

TEST_CASE("Integration: System state EStop (emergency stop)", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: system-level emergency stop
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.9f;
    driver_in.brake = 0.1f;
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::EStop;  // System emergency stop

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Engine zero, Brake maximum (same as estop=true)
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
    REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("Integration: Input clamping for Engine and Brake", "[integration]") {
    IntegrationTestFixture fixture;

    // Setup: out-of-range inputs (should be clamped)
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.8f;  // > 1.0
    driver_in.brake = -0.5f;    // < 0.0
    driver_in.steer = 0.0f;

    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;

    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);

    // Execute: Engine + Brake cycle
    Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

    // Verify: Both should clamp to valid range
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(2.0f, 0.01f));  // Throttle clamped to 1.0 → 2.0 accel
    REQUIRE(cmd.brake_decel_cmd == 0.0f);  // Brake clamped to 0.0 → 0.0 decel
}

TEST_CASE("Integration: Multiple execution cycles (state retention)", "[integration]") {
    IntegrationTestFixture fixture;

    // Cycle 1: Acceleration
    {
        Rte::DriverInput driver_in{.throttle = 0.7f, .brake = 0.0f};
        Rte::Safety safety{.estop = false, .system_state = Rte::SystemState::Normal};
        Mock_Rte_SetDriverInput(driver_in);
        Mock_Rte_SetSafety(safety);
        Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

        auto cmd = Mock_Rte_Written_ActuatorCmd();
        REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.4f, 0.01f));
        REQUIRE(cmd.brake_decel_cmd == 0.0f);
    }

    // Cycle 2: Braking (overrides previous cycle)
    {
        Rte::DriverInput driver_in{.throttle = 0.0f, .brake = 0.8f};
        Rte::Safety safety{.estop = false, .system_state = Rte::SystemState::Normal};
        Mock_Rte_SetDriverInput(driver_in);
        Mock_Rte_SetSafety(safety);
        Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

        auto cmd = Mock_Rte_Written_ActuatorCmd();
        REQUIRE(cmd.drive_accel_cmd == 0.0f);
        REQUIRE_THAT(cmd.brake_decel_cmd, WithinAbs(3.2f, 0.01f));  // 0.8 * 4.0
    }

    // Cycle 3: Coast
    {
        Rte::DriverInput driver_in{.throttle = 0.0f, .brake = 0.0f};
        Rte::Safety safety{.estop = false, .system_state = Rte::SystemState::Normal};
        Mock_Rte_SetDriverInput(driver_in);
        Mock_Rte_SetSafety(safety);
        Swc::Integration::Test::ExecuteEngineAndBrakeCycle(0.01);

        auto cmd = Mock_Rte_Written_ActuatorCmd();
        REQUIRE(cmd.drive_accel_cmd == 0.0f);
        REQUIRE(cmd.brake_decel_cmd == 0.0f);
    }
}
