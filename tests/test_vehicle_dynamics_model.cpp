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

// ============================================================
// Lateral dynamics (kinematic bicycle model) and the combined step
// ============================================================
#include <cmath>

TEST_CASE("VehicleDynamics: default params are the ones the vehicle SWC runs with") {
  Model::VehicleParams p;
  REQUIRE_THAT(p.wheel_radius_m, WithinAbs(0.03f, 1e-6f));
  REQUIRE_THAT(p.wheelbase_m, WithinAbs(0.20f, 1e-6f));
  REQUIRE_THAT(p.linear_drag, WithinAbs(0.15f, 1e-6f));
  REQUIRE_THAT(p.max_speed_mps, WithinAbs(3.0f, 1e-6f));
  REQUIRE_THAT(p.estop_decel_mps2, WithinAbs(6.0f, 1e-6f));
}

TEST_CASE("StepLateral: zero steering keeps the heading and drives straight") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 1.0f;
  for (int i = 0; i < 100; ++i) s = Model::StepLateral(s, 0.01f, 0.0f, p);
  REQUIRE_THAT(s.yaw, WithinAbs(0.0f, 1e-6f));
  REQUIRE_THAT(s.yaw_rate, WithinAbs(0.0f, 1e-6f));
  REQUIRE_THAT(s.y, WithinAbs(0.0f, 1e-6f));
  REQUIRE_THAT(s.x, WithinAbs(1.0f, 1e-4f));
}

TEST_CASE("StepLateral: positive steer turns LEFT (counter-clockwise, yaw increases)") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 1.0f;
  for (int i = 0; i < 50; ++i) s = Model::StepLateral(s, 0.01f, 0.2f, p);
  REQUIRE(s.yaw_rate > 0.0f);
  REQUIRE(s.yaw > 0.0f);
  REQUIRE(s.y > 0.0f);  // heading +x, turning left moves toward +y
}

TEST_CASE("StepLateral: negative steer turns right and is the mirror image") {
  Model::VehicleParams p;
  Model::VehicleState l, r;
  l.v = r.v = 1.0f;
  for (int i = 0; i < 80; ++i) {
    l = Model::StepLateral(l, 0.01f, 0.25f, p);
    r = Model::StepLateral(r, 0.01f, -0.25f, p);
  }
  REQUIRE(r.yaw < 0.0f);
  REQUIRE_THAT(l.x, WithinAbs(r.x, 1e-5f));
  REQUIRE_THAT(l.y, WithinAbs(-r.y, 1e-5f));
  REQUIRE_THAT(l.yaw, WithinAbs(-r.yaw, 1e-6f));
}

TEST_CASE("StepLateral: a stationary vehicle does not turn or move") {
  Model::VehicleParams p;
  Model::VehicleState s;  // v = 0
  s.x = 1.0f; s.y = 2.0f; s.yaw = 0.5f;
  auto s2 = Model::StepLateral(s, 0.01f, 0.4f, p);
  REQUIRE_THAT(s2.yaw_rate, WithinAbs(0.0f, 1e-9f));
  REQUIRE_THAT(s2.yaw, WithinAbs(0.5f, 1e-9f));
  REQUIRE_THAT(s2.x, WithinAbs(1.0f, 1e-9f));
  REQUIRE_THAT(s2.y, WithinAbs(2.0f, 1e-9f));
}

TEST_CASE("StepLateral: yaw rate follows v/L * tan(steer); a longer wheelbase turns less") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 0.6f;
  auto s2 = Model::StepLateral(s, 0.01f, 0.3f, p);
  REQUIRE_THAT(s2.yaw_rate, WithinAbs((0.6f / 0.20f) * std::tan(0.3f), 1e-5f));
  p.wheelbase_m = 0.40f;
  auto s3 = Model::StepLateral(s, 0.01f, 0.3f, p);
  REQUIRE_THAT(s3.yaw_rate, WithinAbs(s2.yaw_rate / 2.0f, 1e-5f));
}

TEST_CASE("StepLateral: the position moves along the (updated) heading") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 1.0f;
  s.yaw = static_cast<float>(M_PI) / 2.0f;  // facing +y
  auto s2 = Model::StepLateral(s, 0.01f, 0.0f, p);
  REQUIRE_THAT(s2.x, WithinAbs(0.0f, 1e-4f));
  REQUIRE_THAT(s2.y, WithinAbs(0.01f, 1e-4f));
}

TEST_CASE("StepLateral: does not touch the longitudinal state") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.t = 1.5f; s.v = 1.0f; s.wheel_omega = 33.0f;
  auto s2 = Model::StepLateral(s, 0.01f, 0.2f, p);
  REQUIRE(s2.t == 1.5f);
  REQUIRE(s2.v == 1.0f);
  REQUIRE(s2.wheel_omega == 33.0f);
}

TEST_CASE("StepVehicle: equals the longitudinal step followed by the lateral step") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 0.5f;
  auto expected = Model::StepLateral(Model::StepLongitudinal(s, 0.01f, 1.5f, 0.2f, false, p), 0.01f, 0.3f, p);
  auto got = Model::StepVehicle(s, 0.01f, 1.5f, 0.2f, 0.3f, false, p);
  REQUIRE(got.t == expected.t);
  REQUIRE(got.v == expected.v);
  REQUIRE(got.x == expected.x);
  REQUIRE(got.y == expected.y);
  REQUIRE(got.yaw == expected.yaw);
  REQUIRE(got.yaw_rate == expected.yaw_rate);
  REQUIRE(got.wheel_omega == expected.wheel_omega);
}

TEST_CASE("StepVehicle: estop stops the car and with it the turning") {
  Model::VehicleParams p;
  Model::VehicleState s;
  s.v = 1.0f;
  for (int i = 0; i < 100; ++i) s = Model::StepVehicle(s, 0.01f, 2.0f, 0.0f, 0.3f, true, p);
  REQUIRE(s.v == 0.0f);
  REQUIRE_THAT(s.yaw_rate, WithinAbs(0.0f, 1e-9f));
}
