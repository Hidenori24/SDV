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

// ============================================================
// Watchdog (alive supervision)
// ============================================================
namespace {
using Bsw::Diag::Component;

// One 100 ms safety cycle in which the listed components report alive 10 times (10 ms tasks).
void Cycle(bool engine, bool brake, bool steering, bool vd)
{
    for (int i = 0; i < 10; ++i) {
        if (engine) Bsw::Diag::ReportAlive(Component::Engine);
        if (brake) Bsw::Diag::ReportAlive(Component::Brake);
        if (steering) Bsw::Diag::ReportAlive(Component::Steering);
        if (vd) Bsw::Diag::ReportAlive(Component::VehicleDynamics);
    }
    Swc::Safety::Main100ms(0.1);
}
void Cycles(int n, bool e, bool b, bool s, bool v) { for (int i = 0; i < n; ++i) Cycle(e, b, s, v); }
Rte::Safety Sf() { return Rte::Rte_Read_Safety(); }
} // namespace

TEST_CASE("Watchdog: healthy system stays Normal indefinitely", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(200, true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::Normal);
    REQUIRE_FALSE(Sf().estop);
}

TEST_CASE("Watchdog: one silent SWC -> Degraded after the timeout, not before", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(5, true, true, true, true);
    Cycles(10, false, true, true, true); // 1000 ms of silence: still within the timeout
    REQUIRE(Sf().system_state == Rte::SystemState::Normal);
    Cycle(false, true, true, true);      // 1100 ms
    REQUIRE(Sf().system_state == Rte::SystemState::Degraded);
    REQUIRE_FALSE(Sf().estop);
}

TEST_CASE("Watchdog: Degraded recovers when the SWC comes back", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(15, true, false, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::Degraded);
    Cycle(true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::Normal);
}

TEST_CASE("Watchdog: two silent SWCs -> EStop with estop flag", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(12, false, false, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::EStop);
    REQUIRE(Sf().estop);
}

TEST_CASE("Watchdog: EStop from the watchdog is latched even if SWCs recover", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(12, false, true, false, true);
    REQUIRE(Sf().estop);
    Cycles(50, true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::EStop);
    REQUIRE(Sf().estop);
    // an external "clear" cannot release the latch; only Init() (reset) does
    Rte::Safety req{};
    Rte::Rte_Write_Safety(req);
    Cycle(true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::EStop);
    Swc::Safety::Init();
    Rte::Rte_Write_Safety(Rte::Safety{});
    Cycle(true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::Normal);
}

TEST_CASE("Watchdog: external estop works regardless of heartbeats", "[safety_swc][watchdog]") {
    SafetySwcFixture f;
    Cycles(3, true, true, true, true);
    Rte::Safety req{}; req.estop = true;
    Rte::Rte_Write_Safety(req);
    Cycle(true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::EStop);
    // external request released -> back to Normal (no watchdog latch involved)
    req.estop = false;
    Rte::Rte_Write_Safety(req);
    Cycle(true, true, true, true);
    REQUIRE(Sf().system_state == Rte::SystemState::Normal);
}
