#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "model/steering_model.h"

using Catch::Matchers::WithinAbs;

TEST_CASE("SteeringModel: converges from 0 toward target", "[steering_model]") {
    Model::SteeringParams p{};
    float a = 0.0f;
    float prev = a;
    for (int i = 0; i < 100; ++i) {
        a = Model::StepSteeringDynamics(a, 0.4f, 0.01f, p);
        REQUIRE(a >= prev);
        REQUIRE(a <= 0.4f);
        prev = a;
    }
    // 1.0 s = ~6.7 tau -> essentially converged
    REQUIRE_THAT(a, WithinAbs(0.4f, 0.001f));
}

TEST_CASE("SteeringModel: converges from target back to 0", "[steering_model]") {
    Model::SteeringParams p{};
    float a = 0.4f;
    for (int i = 0; i < 100; ++i) {
        a = Model::StepSteeringDynamics(a, 0.0f, 0.01f, p);
        REQUIRE(a >= 0.0f);
    }
    REQUIRE_THAT(a, WithinAbs(0.0f, 0.001f));
}

TEST_CASE("SteeringModel: target beyond max angle is saturated", "[steering_model]") {
    Model::SteeringParams p{};
    float a = 0.0f;
    for (int i = 0; i < 1000; ++i) a = Model::StepSteeringDynamics(a, 5.0f, 0.01f, p);
    REQUIRE_THAT(a, WithinAbs(p.max_steer_angle_rad, 1e-4f));

    a = 0.0f;
    for (int i = 0; i < 1000; ++i) a = Model::StepSteeringDynamics(a, -5.0f, 0.01f, p);
    REQUIRE_THAT(a, WithinAbs(-p.max_steer_angle_rad, 1e-4f));
}

TEST_CASE("SteeringModel: reaches 63.2% of target after dt = tau", "[steering_model]") {
    Model::SteeringParams p{};
    const float a = Model::StepSteeringDynamics(0.0f, 0.4f, p.time_constant_s, p);
    REQUIRE_THAT(a, WithinAbs(0.4f * 0.6321206f, 1e-4f));
}

TEST_CASE("SteeringModel: dt = 0 leaves the angle unchanged", "[steering_model]") {
    Model::SteeringParams p{};
    REQUIRE(Model::StepSteeringDynamics(0.123f, 0.4f, 0.0f, p) == 0.123f);
}

TEST_CASE("SteeringModel: large dt does not overshoot", "[steering_model]") {
    Model::SteeringParams p{};
    const float a = Model::StepSteeringDynamics(0.0f, 0.4f, 100.0f, p);
    REQUIRE(a <= 0.4f);
    REQUIRE_THAT(a, WithinAbs(0.4f, 1e-4f));
}

TEST_CASE("SteeringModel: ComputeSteerTarget scales, clamps and honors estop", "[steering_model]") {
    Model::SteeringParams p{};
    REQUIRE_THAT(Model::ComputeSteerTarget(0.5f, false, p), WithinAbs(0.2f, 1e-6f));
    REQUIRE_THAT(Model::ComputeSteerTarget(3.0f, false, p), WithinAbs(0.4f, 1e-6f));
    REQUIRE_THAT(Model::ComputeSteerTarget(-3.0f, false, p), WithinAbs(-0.4f, 1e-6f));
    REQUIRE(Model::ComputeSteerTarget(1.0f, true, p) == 0.0f);
}
