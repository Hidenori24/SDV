#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "model/brake_model.h"

using Catch::Matchers::WithinAbs;

// ============================================================
// Brake Model Tests
// ============================================================

TEST_CASE("BrakeModel: brake=0 produces zero deceleration", "[brake_model]") {
    Model::BrakeParams params;
    
    float decel = Model::ComputeBrakeDecel(0.0f, false, params);
    
    REQUIRE(decel == 0.0f);
}

TEST_CASE("BrakeModel: brake=0.5 produces half max deceleration", "[brake_model]") {
    Model::BrakeParams params{.max_decel_mps2 = 4.0f};
    
    float decel = Model::ComputeBrakeDecel(0.5f, false, params);
    
    REQUIRE_THAT(decel, WithinAbs(2.0f, 0.01f));
}

TEST_CASE("BrakeModel: brake=1.0 produces max deceleration", "[brake_model]") {
    Model::BrakeParams params{.max_decel_mps2 = 4.0f};
    
    float decel = Model::ComputeBrakeDecel(1.0f, false, params);
    
    REQUIRE_THAT(decel, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeModel: estop=true forces max deceleration", "[brake_model]") {
    Model::BrakeParams params{.estop_max_decel_mps2 = 4.0f};
    
    // Even with brake=0, estop should force maximum deceleration
    float decel_zero = Model::ComputeBrakeDecel(0.0f, true, params);
    REQUIRE_THAT(decel_zero, WithinAbs(4.0f, 0.01f));
    
    // Even with brake=0.5, estop should force maximum deceleration
    float decel_half = Model::ComputeBrakeDecel(0.5f, true, params);
    REQUIRE_THAT(decel_half, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeModel: brake > 1.0 is clamped to max", "[brake_model]") {
    Model::BrakeParams params{.max_decel_mps2 = 4.0f};
    
    float decel = Model::ComputeBrakeDecel(1.5f, false, params);
    
    // Should be clamped to 1.0, so result is 1.0 * 4.0 = 4.0
    REQUIRE_THAT(decel, WithinAbs(4.0f, 0.01f));
}

TEST_CASE("BrakeModel: negative brake is clamped to zero", "[brake_model]") {
    Model::BrakeParams params{.max_decel_mps2 = 4.0f};
    
    float decel = Model::ComputeBrakeDecel(-0.5f, false, params);
    
    // Should be clamped to 0.0, so result is 0.0
    REQUIRE(decel == 0.0f);
}

TEST_CASE("BrakeModel: custom max_decel parameter", "[brake_model]") {
    Model::BrakeParams params{.max_decel_mps2 = 5.0f};
    
    float decel = Model::ComputeBrakeDecel(0.6f, false, params);
    
    // 0.6 * 5.0 = 3.0
    REQUIRE_THAT(decel, WithinAbs(3.0f, 0.01f));
}

TEST_CASE("BrakeModel: estop with custom estop_max_decel", "[brake_model]") {
    Model::BrakeParams params{
        .max_decel_mps2 = 4.0f,
        .estop_max_decel_mps2 = 6.0f
    };
    
    float decel = Model::ComputeBrakeDecel(0.5f, true, params);
    
    // Should use estop_max_decel_mps2, not max_decel_mps2
    REQUIRE_THAT(decel, WithinAbs(6.0f, 0.01f));
}
