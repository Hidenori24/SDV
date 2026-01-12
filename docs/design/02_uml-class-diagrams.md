# UML Class Diagrams & Entity Relations

## 1. Model層 クラス図

```
┌─────────────────────────────────────┐
│        Model Namespace              │
├─────────────────────────────────────┤
│                                     │
│  ┌──────────────────────┐          │
│  │ EngineParams         │          │
│  ├──────────────────────┤          │
│  │ - max_accel_mps2: f  │          │
│  └──────────────────────┘          │
│           ↑                         │
│           │ uses                    │
│           │                         │
│  ┌──────────────────────────────┐  │
│  │ ComputeDriveAccel()          │  │
│  │ (pure function)              │  │
│  │ Input: throttle, estop       │  │
│  │ Output: drive_accel_cmd      │  │
│  └──────────────────────────────┘  │
│                                     │
│  ┌──────────────────────┐          │
│  │ BrakeParams          │          │
│  ├──────────────────────┤          │
│  │ - max_decel_mps2: f  │          │
│  └──────────────────────┘          │
│           ↑                         │
│           │ uses                    │
│           │                         │
│  ┌──────────────────────────────┐  │
│  │ ComputeBrakeDecel()          │  │
│  │ (pure function)              │  │
│  │ Input: brake, estop          │  │
│  │ Output: brake_decel_cmd      │  │
│  └──────────────────────────────┘  │
│                                     │
│  ┌──────────────────────┐          │
│  │ SteeringParams       │          │
│  ├──────────────────────┤          │
│  │ - max_angle: f       │          │
│  │ - time_constant: f   │          │
│  └──────────────────────┘          │
│           ↑                         │
│           │ uses                    │
│           │                         │
│  ┌──────────────────────────────┐  │
│  │ StepSteeringDynamics()       │  │
│  │ (pure function)              │  │
│  │ Input: current, target, τ    │  │
│  │ Output: new_angle            │  │
│  └──────────────────────────────┘  │
│                                     │
│  ┌──────────────────────────────┐  │
│  │ VehicleState                 │  │
│  ├──────────────────────────────┤  │
│  │ - t, v, x, y: float          │  │
│  │ - yaw, yaw_rate: float       │  │
│  │ - wheel_omega: float         │  │
│  └──────────────────────────────┘  │
│           ↑                         │
│           │ updated by             │
│           │                         │
│  ┌──────────────────────────────┐  │
│  │ StepLongitudinal()           │  │
│  │ StepLateral()                │  │
│  │ (pure functions)             │  │
│  │ Input: state, cmd, estop     │  │
│  │ Output: new_state            │  │
│  └──────────────────────────────┘  │
│                                     │
└─────────────────────────────────────┘
```

---

## 2. SWC層 クラス図

```
┌─────────────────────────────────────────┐
│     Swc Namespace                       │
├─────────────────────────────────────────┤
│                                         │
│  ┌──────────────────────────────┐      │
│  │ EngineSWC                    │      │
│  ├──────────────────────────────┤      │
│  │ - g_params: EngineParams     │      │
│  ├──────────────────────────────┤      │
│  │ + Init()                     │      │
│  │ + Main10ms(dt)               │      │
│  │ + Version()                  │      │
│  ├──────────────────────────────┤      │
│  │ Actions:                     │      │
│  │  1. RTE_Read_DriverInput()   │      │
│  │  2. RTE_Read_Safety()        │      │
│  │  3. Model::Compute...()      │      │
│  │  4. RTE_Write_ActuatorCmd()  │      │
│  └──────────────────────────────┘      │
│                 ↓ (uses)               │
│       Model::ComputeDriveAccel()       │
│                                         │
│  ┌──────────────────────────────┐      │
│  │ BrakeSWC (similar pattern)   │      │
│  └──────────────────────────────┘      │
│                 ↓ (uses)               │
│       Model::ComputeBrakeDecel()       │
│                                         │
│  ┌──────────────────────────────┐      │
│  │ SteeringSWC                  │      │
│  ├──────────────────────────────┤      │
│  │ - g_params: SteeringParams   │      │
│  │ - g_steer_angle: float       │ ← state
│  ├──────────────────────────────┤      │
│  │ + Init()                     │      │
│  │ + Main10ms(dt)               │      │
│  │ + Version()                  │      │
│  └──────────────────────────────┘      │
│                 ↓ (uses)               │
│      Model::StepSteeringDynamics()     │
│                                         │
│  ┌──────────────────────────────┐      │
│  │ VehicleDynamicsSWC           │      │
│  ├──────────────────────────────┤      │
│  │ - g_params: VehicleParams    │      │
│  ├──────────────────────────────┤      │
│  │ + Init()                     │      │
│  │ + Main10ms(dt)               │      │
│  │ + Version()                  │      │
│  ├──────────────────────────────┤      │
│  │ Actions:                     │      │
│  │  1. RTE_Read_ActuatorCmd()   │      │
│  │  2. RTE_Read_VehicleState()  │      │
│  │  3. Model::StepLongitudinal()│      │
│  │  4. Model::StepLateral()     │      │
│  │  5. RTE_Write_VehicleState() │      │
│  └──────────────────────────────┘      │
│                                         │
└─────────────────────────────────────────┘
```

---

## 3. RTE Signal依存関係

```
RTE Global Buffer
    │
    ├─→ DriverInput {throttle, brake, steer}
    │       ↑
    │       │ read by
    │   ┌───┴────────────────────┐
    │   │                        │
    ├─→ EngineSWC          SteeringSWC
    │   │                        │
    │   └───┬────────────────────┘
    │       ↓ write
    │   ActuatorCmd {drive_accel, brake_decel, steer_angle_cmd}
    │       ↑
    │       │ read by
    │
    ├─→ VehicleDynamicsSWC
    │   │
    │   ├─ read: ActuatorCmd
    │   ├─ read: VehicleState (previous)
    │   └─ write: VehicleState (next)
    │
    ├─→ VehicleState {v, x, y, yaw, ...}
    │       ↑
    │       │ read by
    │   ┌───┴──────────────────┐
    │   │                      │
    ├─→ Logging            Safety
    │   (CSV output)      (Heartbeat)
    │
    └─→ Safety {estop, system_state}
        ↑
        │ read by
        └──→ EngineSWC, BrakeSWC, SteeringSWC
```

---

## 4. 実装パターン：Model層 vs SWC層

### Model層（src/model/*.h）- Pure Function Pattern

```cpp
namespace Model {
    // ============ Params ============
    struct EngineParams {
        float max_accel_mps2 = 2.0f;
    };
    
    // ============ Pure Function ============
    inline float ComputeDriveAccel(
        float throttle_0_1, 
        bool estop, 
        const EngineParams& p)
    {
        // No side effects
        // No global state modification
        // Deterministic: same input → same output
        
        if (estop) return 0.0f;
        float th = std::clamp(throttle_0_1, 0.0f, 1.0f);
        return th * p.max_accel_mps2;
    }
}
```

### SWC層（src/swc/*.cpp）- RTE I/O Pattern

```cpp
namespace Swc::Engine {
    static Model::EngineParams g_params{};
    
    void Init() {
        // One-time initialization
    }
    
    void Main10ms(double dt_s) {
        // ============ 1. RTE Read ============
        const auto in = Rte::Rte_Read_DriverInput();
        const auto sf = Rte::Rte_Read_Safety();
        auto cmd = Rte::Rte_Read_ActuatorCmd();
        
        // ============ 2. Prepare input ============
        bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
        
        // ============ 3. Call Model ============
        cmd.drive_accel_cmd = Model::ComputeDriveAccel(
            in.throttle, 
            estop, 
            g_params
        );
        
        // ============ 4. RTE Write ============
        Rte::Rte_Write_ActuatorCmd(cmd);
    }
}
```

---

## 5. テスト層 Pattern

### Model層テスト（tests/test_engine_model.cpp）

```cpp
#include <catch2/catch_test_macros.hpp>
#include "model/engine_model.h"

TEST_CASE("EngineModel: basic computation") {
    Model::EngineParams p{.max_accel_mps2 = 2.0f};
    
    // Test case 1: normal operation
    float result = Model::ComputeDriveAccel(0.5f, false, p);
    REQUIRE(result == Approx(1.0f));
    
    // Test case 2: e-stop override
    result = Model::ComputeDriveAccel(1.0f, true, p);
    REQUIRE(result == 0.0f);
}
```

### SWC層テスト（tests/test_engine_swc.cpp + mock_rte.h）

```cpp
#include <catch2/catch_test_macros.hpp>
#include "mock_rte.h"
#include "swc/engine_swc.h"

TEST_CASE("EngineSWC: RTE integration") {
    // Setup Mock RTE
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    
    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // Execute SWC
    Swc::Engine::Main10ms(0.01);
    
    // Verify output
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == Approx(1.0f));
}
```

---

## 6. 型と構造体の関係図

```
┌─────────────────────────────────┐
│   Core Data Types               │
├─────────────────────────────────┤
│                                 │
│  float throttle (0..1)          │
│  float brake (0..1)             │
│  float steer (-1..1)            │
│         ↓                       │
│    DriverInput struct           │
│         ↓                       │
│  [RTE Buffer]                   │
│         ↓                       │
│  EngineSWC reads                │
│  BrakeSWC reads                 │
│  SteeringSWC reads              │
│         ↓ (writes)              │
│    ActuatorCmd struct           │
│  ├─ drive_accel_cmd (m/s²)      │
│  ├─ brake_decel_cmd (m/s²)      │
│  └─ steer_angle_cmd (rad)       │
│         ↓                       │
│  [RTE Buffer]                   │
│         ↓                       │
│  VehicleDynamicsSWC reads       │
│         ↓ (updates)             │
│    VehicleState struct          │
│  ├─ v (m/s)                     │
│  ├─ x, y (m)                    │
│  ├─ yaw (rad)                   │
│  ├─ yaw_rate (rad/s)            │
│  └─ wheel_omega (rad/s)         │
│         ↓                       │
│  [RTE Buffer]                   │
│         ↓                       │
│  Logging, Safety reads          │
│                                 │
└─────────────────────────────────┘
```
