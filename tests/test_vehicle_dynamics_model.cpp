#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "model/vehicledynamics_model.h"

using Catch::Matchers::WithinAbs;

TEST_CASE("VehicleDynamics: speed increases with positive accel") {
  Model::VehicleParams p;
  p.wheel_radius_m = 0.05f;
  p.linear_drag = 0.0f;
  p.max_speed_mps = 100.0f;

  Model::VehicleState s;
  const float dt = 0.01f;

  auto s2 = Model::StepLongitudinal(s, dt, /*drive=*/2.0f, /*brake=*/0.0f, /*estop=*/false, p);
  REQUIRE(s2.v > s.v);
  REQUIRE(s2.wheel_omega > s.wheel_omega);
}

TEST_CASE("VehicleDynamics: braking never makes v negative") {
  Model::VehicleParams p;
  p.max_speed_mps = 100.0f;

  Model::VehicleState s;
  s.v = 0.1f;

  auto s2 = Model::StepLongitudinal(s, 0.01f, 0.0f, 100.0f, false, p);
  REQUIRE(s2.v >= 0.0f);
}
