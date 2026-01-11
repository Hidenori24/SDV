#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "model/engine_model.h"

using Catch::Matchers::WithinAbs;

TEST_CASE("EngineModel: estop forces zero accel") {
  Model::EngineParams p{.max_accel_mps2 = 2.0f};
  REQUIRE(Model::ComputeDriveAccel(1.0f, true, p) == 0.0f);
}

TEST_CASE("EngineModel: throttle is clamped and scaled") {
  Model::EngineParams p{.max_accel_mps2 = 2.0f};
  REQUIRE(Model::ComputeDriveAccel(0.0f, false, p) == 0.0f);
  REQUIRE_THAT(Model::ComputeDriveAccel(0.5f, false, p), WithinAbs(1.0f, 1e-5f));
  REQUIRE_THAT(Model::ComputeDriveAccel(2.0f, false, p), WithinAbs(2.0f, 1e-5f));   // clamp
  REQUIRE_THAT(Model::ComputeDriveAccel(-1.0f, false, p), WithinAbs(0.0f, 1e-5f));  // clamp
}
