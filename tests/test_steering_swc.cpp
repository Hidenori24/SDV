#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "mock_rte.h"
#include "rte/rte.h"
#include "model/steering_model.h"

using Catch::Matchers::WithinAbs;

namespace Swc::Steering::Test {

static Model::SteeringParams g_params{};
static float g_steer_angle = 0.0f;

void Init() { g_steer_angle = 0.0f; }

void Main10ms_MockRTE(double dt_s)
{
  const auto in = Mock_Rte_Read_DriverInput();
  const auto sf = Mock_Rte_Read_Safety();

  auto cmd = Mock_Rte_Read_ActuatorCmd();

  const bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;

  g_steer_angle = Model::StepSteerAngle(in.steer, estop, g_steer_angle, static_cast<float>(dt_s), g_params);
  cmd.steer_angle_cmd = g_steer_angle;

  Mock_Rte_Write_ActuatorCmd(cmd);
}

} // namespace Swc::Steering::Test

struct SteeringSwcTestFixture {
  SteeringSwcTestFixture() {
    Mock_Rte_Reset();
    Swc::Steering::Test::Init();
  }

  ~SteeringSwcTestFixture() {
    Mock_Rte_Reset();
  }
};

TEST_CASE("SteeringSWC: steer=1.0 produces positive angle", "[steering_swc]") {
  SteeringSwcTestFixture fixture;

  Rte::DriverInput driver_in{};
  driver_in.steer = 1.0f;

  Rte::Safety safety{};
  safety.estop = false;
  safety.system_state = Rte::SystemState::Normal;

  Mock_Rte_SetDriverInput(driver_in);
  Mock_Rte_SetSafety(safety);

  Swc::Steering::Test::Main10ms_MockRTE(0.01);

  const float expected = 0.40f * (0.01f / 0.15f);
  auto cmd = Mock_Rte_Written_ActuatorCmd();
  REQUIRE_THAT(cmd.steer_angle_cmd, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringSWC: steer=-1.0 produces negative angle", "[steering_swc]") {
  SteeringSwcTestFixture fixture;

  Rte::DriverInput driver_in{};
  driver_in.steer = -1.0f;

  Rte::Safety safety{};
  safety.estop = false;
  safety.system_state = Rte::SystemState::Normal;

  Mock_Rte_SetDriverInput(driver_in);
  Mock_Rte_SetSafety(safety);

  Swc::Steering::Test::Main10ms_MockRTE(0.01);

  const float expected = -0.40f * (0.01f / 0.15f);
  auto cmd = Mock_Rte_Written_ActuatorCmd();
  REQUIRE_THAT(cmd.steer_angle_cmd, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringSWC: steer is clamped", "[steering_swc]") {
  SteeringSwcTestFixture fixture;

  Rte::DriverInput driver_in{};
  driver_in.steer = 2.0f;

  Rte::Safety safety{};
  safety.estop = false;
  safety.system_state = Rte::SystemState::Normal;

  Mock_Rte_SetDriverInput(driver_in);
  Mock_Rte_SetSafety(safety);

  Swc::Steering::Test::Main10ms_MockRTE(0.01);

  const float expected = 0.40f * (0.01f / 0.15f);
  auto cmd = Mock_Rte_Written_ActuatorCmd();
  REQUIRE_THAT(cmd.steer_angle_cmd, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringSWC: estop decays toward zero", "[steering_swc]") {
  SteeringSwcTestFixture fixture;

  Rte::DriverInput driver_in{};
  driver_in.steer = 1.0f;

  Rte::Safety safety{};
  safety.estop = false;
  safety.system_state = Rte::SystemState::Normal;

  Mock_Rte_SetDriverInput(driver_in);
  Mock_Rte_SetSafety(safety);

  Swc::Steering::Test::Main10ms_MockRTE(0.01);

  safety.estop = true;
  Mock_Rte_SetSafety(safety);

  Swc::Steering::Test::Main10ms_MockRTE(0.01);

  auto cmd = Mock_Rte_Written_ActuatorCmd();
  REQUIRE(cmd.steer_angle_cmd < 0.40f);
  REQUIRE(cmd.steer_angle_cmd > 0.0f);
}
