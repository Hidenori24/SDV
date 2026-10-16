#include <catch2/catch_test_macros.hpp>
#include "model/safety_logic.h"

using namespace Model;

TEST_CASE("SafetyLogic: all alive -> Normal", "[safety_logic]") {
    REQUIRE(ComputeSafetyState(HeartbeatStatus{}) == SafetyState::Normal);
}

TEST_CASE("SafetyLogic: one dead SWC -> Degraded (each SWC)", "[safety_logic]") {
    HeartbeatStatus a; a.engine_ok = false;
    HeartbeatStatus b; b.brake_ok = false;
    HeartbeatStatus c; c.steering_ok = false;
    HeartbeatStatus d; d.vehicledynamics_ok = false;
    for (const auto& hb : {a, b, c, d}) {
        REQUIRE(CountFailed(hb) == 1);
        REQUIRE(ComputeSafetyState(hb) == SafetyState::Degraded);
    }
}

TEST_CASE("SafetyLogic: two or more dead SWCs -> EStop", "[safety_logic]") {
    HeartbeatStatus hb; hb.engine_ok = false; hb.brake_ok = false;
    REQUIRE(ComputeSafetyState(hb) == SafetyState::EStop);
    hb.steering_ok = false; hb.vehicledynamics_ok = false;
    REQUIRE(CountFailed(hb) == 4);
    REQUIRE(ComputeSafetyState(hb) == SafetyState::EStop);
}

TEST_CASE("HeartbeatMonitor: advancing counter keeps the SWC alive", "[safety_logic]") {
    SafetyParams p;
    HeartbeatMonitor m;
    for (uint64_t c = 1; c <= 100; ++c) {
        REQUIRE(UpdateHeartbeat(m, c, 100, p));
        REQUIRE(m.silent_ms == 0);
    }
}

TEST_CASE("HeartbeatMonitor: silence beyond the timeout is detected", "[safety_logic]") {
    SafetyParams p; // 1000 ms
    HeartbeatMonitor m;
    REQUIRE(UpdateHeartbeat(m, 1, 100, p));
    // 10 silent cycles = 1000 ms: still within the timeout (not strictly greater)
    for (int i = 0; i < 10; ++i) REQUIRE(UpdateHeartbeat(m, 1, 100, p));
    REQUIRE(m.silent_ms == 1000);
    // the 11th silent cycle exceeds it
    REQUIRE_FALSE(UpdateHeartbeat(m, 1, 100, p));
}

TEST_CASE("HeartbeatMonitor: counter advancing again clears the failure", "[safety_logic]") {
    SafetyParams p;
    HeartbeatMonitor m;
    UpdateHeartbeat(m, 1, 100, p);
    for (int i = 0; i < 12; ++i) UpdateHeartbeat(m, 1, 100, p);
    REQUIRE_FALSE(UpdateHeartbeat(m, 1, 100, p));
    REQUIRE(UpdateHeartbeat(m, 2, 100, p));
    REQUIRE(m.silent_ms == 0);
}

TEST_CASE("HeartbeatMonitor: custom timeout is honored", "[safety_logic]") {
    SafetyParams p; p.heartbeat_timeout_ms = 300;
    HeartbeatMonitor m;
    UpdateHeartbeat(m, 1, 100, p);
    REQUIRE(UpdateHeartbeat(m, 1, 100, p));
    REQUIRE(UpdateHeartbeat(m, 1, 100, p));
    REQUIRE(UpdateHeartbeat(m, 1, 100, p)); // 300 ms
    REQUIRE_FALSE(UpdateHeartbeat(m, 1, 100, p)); // 400 ms
}
