#include <catch2/catch_test_macros.hpp>
#include "bsw/diag.h"
#include "rte/rte.h"
#include "swc/engine_swc.h"
#include "swc/brake_swc.h"
#include "swc/steering_swc.h"
#include "swc/vehicledynamics_swc.h"
#include "swc/safety_swc.h"

// End-to-end: real SWCs + real RTE + real Safety watchdog on the 10 ms / 100 ms schedule of main.cpp.
namespace {
struct Stalls { bool engine = false, brake = false, steering = false, vd = false; };

void Init()
{
    Rte::InitDefaults();
    Bsw::Diag::Init();
    Swc::Engine::Init(); Swc::Brake::Init(); Swc::Steering::Init();
    Swc::VehicleDynamics::Init(); Swc::Safety::Init();
}

void Run(int ticks10ms, const Stalls& s, int* tick)
{
    for (int n = 0; n < ticks10ms; ++n, ++*tick) {
        if (!s.engine) Swc::Engine::Main10ms(0.01);
        if (!s.brake) Swc::Brake::Main10ms(0.01);
        if (!s.steering) Swc::Steering::Main10ms(0.01);
        if (!s.vd) Swc::VehicleDynamics::Step10ms(0.01);
        if (*tick % 10 == 0) Swc::Safety::Main100ms(0.1);
    }
}
} // namespace

TEST_CASE("Integration: all SWCs report alive every cycle", "[watchdog_integration]") {
    Init();
    Swc::Engine::Main10ms(0.01); Swc::Brake::Main10ms(0.01);
    Swc::Steering::Main10ms(0.01); Swc::VehicleDynamics::Step10ms(0.01);
    using C = Bsw::Diag::Component;
    REQUIRE(Bsw::Diag::GetAliveCount(C::Engine) == 1);
    REQUIRE(Bsw::Diag::GetAliveCount(C::Brake) == 1);
    REQUIRE(Bsw::Diag::GetAliveCount(C::Steering) == 1);
    REQUIRE(Bsw::Diag::GetAliveCount(C::VehicleDynamics) == 1);
}

TEST_CASE("Integration: no false positives over 60 s of normal operation", "[watchdog_integration]") {
    Init();
    Rte::DriverInput in{}; in.throttle = 0.5f; in.steer = 0.3f;
    Rte::Rte_Write_DriverInput(in);
    int tick = 0;
    Run(6000, {}, &tick);
    REQUIRE(Rte::Rte_Read_Safety().system_state == Rte::SystemState::Normal);
    REQUIRE(Rte::Rte_Read_VehicleState().v > 0.5f);
}

TEST_CASE("Integration: one stalled SWC -> Degraded, vehicle keeps running", "[watchdog_integration]") {
    Init();
    Rte::DriverInput in{}; in.throttle = 0.5f;
    Rte::Rte_Write_DriverInput(in);
    int tick = 0;
    Run(300, {}, &tick);
    Stalls s; s.steering = true;
    Run(1500, s, &tick); // 1.5 s > 1.0 s timeout
    REQUIRE(Rte::Rte_Read_Safety().system_state == Rte::SystemState::Degraded);
    REQUIRE_FALSE(Rte::Rte_Read_Safety().estop);
    REQUIRE(Rte::Rte_Read_VehicleState().v > 0.1f);
}

TEST_CASE("Integration: two stalled SWCs -> EStop and the vehicle stops", "[watchdog_integration]") {
    Init();
    Rte::DriverInput in{}; in.throttle = 1.0f;
    Rte::Rte_Write_DriverInput(in);
    int tick = 0;
    Run(500, {}, &tick);
    REQUIRE(Rte::Rte_Read_VehicleState().v > 0.5f);

    // Steering and Brake stop; Engine and VehicleDynamics keep running.
    // The last drive/brake commands remain in the RTE, so only the EStop path can stop the car.
    Stalls s; s.steering = true; s.brake = true;
    Run(1300, s, &tick);
    REQUIRE(Rte::Rte_Read_Safety().system_state == Rte::SystemState::EStop);
    REQUIRE(Rte::Rte_Read_Safety().estop);

    Run(500, s, &tick);
    // VehicleDynamics applies the extra estop decel itself, so it stops even with a stale drive command
    REQUIRE(Rte::Rte_Read_VehicleState().v == 0.0f);
}
