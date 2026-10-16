#include <catch2/catch_test_macros.hpp>
#include "bsw/diag.h"
#include "rte/rte.h"
#include "swc/safety_swc.h"

// Pins the CURRENT behavior of the Safety SWC skeleton:
// it mirrors Safety.estop into system_state and bumps the system heartbeat.
// Per-SWC heartbeat monitoring (Watchdog -> EStop) is not implemented yet.
namespace {
struct SafetySwcFixture {
    SafetySwcFixture() {
        Rte::InitDefaults();
        Bsw::Diag::Init();
        Swc::Safety::Init();
    }
};
} // namespace

TEST_CASE("SafetySWC: initial state is Normal without estop", "[safety_swc]") {
    SafetySwcFixture f;
    Swc::Safety::Main100ms(0.1);
    const auto sf = Rte::Rte_Read_Safety();
    REQUIRE_FALSE(sf.estop);
    REQUIRE(sf.system_state == Rte::SystemState::Normal);
}

TEST_CASE("SafetySWC: estop request moves system_state to EStop", "[safety_swc]") {
    SafetySwcFixture f;
    Rte::Safety req{};
    req.estop = true;
    Rte::Rte_Write_Safety(req);
    Swc::Safety::Main100ms(0.1);
    const auto sf = Rte::Rte_Read_Safety();
    REQUIRE(sf.estop);
    REQUIRE(sf.system_state == Rte::SystemState::EStop);
}

TEST_CASE("SafetySWC: estop is latched in the RTE across cycles", "[safety_swc]") {
    SafetySwcFixture f;
    Rte::Safety req{};
    req.estop = true;
    Rte::Rte_Write_Safety(req);
    for (int i = 0; i < 5; ++i) Swc::Safety::Main100ms(0.1);
    REQUIRE(Rte::Rte_Read_Safety().system_state == Rte::SystemState::EStop);
}

TEST_CASE("SafetySWC: clearing estop returns to Normal", "[safety_swc]") {
    SafetySwcFixture f;
    Rte::Safety req{};
    req.estop = true;
    Rte::Rte_Write_Safety(req);
    Swc::Safety::Main100ms(0.1);
    req.estop = false;
    Rte::Rte_Write_Safety(req);
    Swc::Safety::Main100ms(0.1);
    REQUIRE(Rte::Rte_Read_Safety().system_state == Rte::SystemState::Normal);
}

TEST_CASE("SafetySWC: each cycle bumps the system heartbeat", "[safety_swc]") {
    SafetySwcFixture f;
    REQUIRE(Bsw::Diag::GetHeartbeat() == 0);
    Swc::Safety::Main100ms(0.1);
    Swc::Safety::Main100ms(0.1);
    REQUIRE(Bsw::Diag::GetHeartbeat() == 2);
}
