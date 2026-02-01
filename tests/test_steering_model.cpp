#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "model/steering_model.h"

using Catch::Matchers::WithinAbs;

TEST_CASE("SteeringModel: zero steer keeps angle", "[steering_model]") {
  Model::SteeringParams params{};
  const float prev = 0.0f;
  const float next = Model::StepSteerAngle(0.0f, false, prev, 0.01f, params);
  REQUIRE_THAT(next, WithinAbs(0.0f, 1e-6f));
}

TEST_CASE("SteeringModel: steer=1.0 steps toward max", "[steering_model]") {
  Model::SteeringParams params{};
  const float prev = 0.0f;
  const float dt = 0.01f;
  const float expected = 0.40f * (dt / params.steer_tau_s);
  const float next = Model::StepSteerAngle(1.0f, false, prev, dt, params);
  REQUIRE_THAT(next, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringModel: steer input is clamped", "[steering_model]") {
  Model::SteeringParams params{};
  const float prev = 0.0f;
  const float dt = 0.01f;
  const float expected = 0.40f * (dt / params.steer_tau_s);
  const float next = Model::StepSteerAngle(2.0f, false, prev, dt, params);
  REQUIRE_THAT(next, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringModel: estop decays toward zero", "[steering_model]") {
  Model::SteeringParams params{};
  const float prev = 0.20f;
  const float dt = 0.01f;
  const float expected = prev + (0.0f - prev) * (dt / params.steer_tau_s);
  const float next = Model::StepSteerAngle(1.0f, true, prev, dt, params);
  REQUIRE_THAT(next, WithinAbs(expected, 1e-4f));
}

TEST_CASE("SteeringModel: output is clamped to max", "[steering_model]") {
  Model::SteeringParams params{};
  const float prev = 1.0f;
  const float dt = 0.01f;
  const float next = Model::StepSteerAngle(1.0f, false, prev, dt, params);
  REQUIRE_THAT(next, WithinAbs(params.max_steer_angle_rad, 1e-6f));
}
