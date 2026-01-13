# SDV システムアーキテクチャ (Mermaid版)

## 1. 全体構成図

```mermaid
graph TD
    subgraph RTE["RTE (Runtime Environment)"]
        direction LR
        DriverInput["DriverInput<br/>├─ throttle<br/>├─ brake<br/>└─ steer"]
        ActuatorCmd["ActuatorCmd<br/>├─ drive_accel_cmd<br/>├─ brake_decel_cmd<br/>└─ steer_angle_cmd"]
        VehicleState["VehicleState<br/>├─ v<br/>├─ x, y<br/>├─ yaw<br/>└─ wheel_omega"]
        Safety["Safety<br/>├─ estop<br/>└─ system_state"]
        DriverInput -.RTE Buffer.- ActuatorCmd
        ActuatorCmd -.RTE Buffer.- VehicleState
        Safety -.RTE Buffer.- DriverInput
    end

    subgraph SWC["SWC Layer (Software Components)"]
        direction LR
        EngineSWC["Engine SWC"]
        BrakeSWC["Brake SWC"]
        SteeringSWC["Steering SWC"]
        VehicleDynSWC["VehicleDynamics SWC"]
    end

    subgraph Model["Model Layer (Pure Functions)"]
        direction LR
        EngineModel["EngineModel<br/>ComputeDriveAccel"]
        BrakeModel["BrakeModel<br/>ComputeDecel"]
        SteeringModel["SteeringModel<br/>StepSteeringDynamics"]
        VehicleModel["VehicleModel<br/>StepLongitudinal<br/>StepLateral"]
    end

    RTE -->|Read/Write| SWC
    SWC -->|Calls| Model
    
    style RTE fill:#ffcccc
    style SWC fill:#ccffcc
    style Model fill:#cce5ff
```

---

## 2. 層の責務分離

### SWC層（Software Component Layer）

**責務**:
- RTE から信号を読み取る
- Model層の関数を呼び出す
- 結果を RTE へ書き込む
- 周期的なタスク実行管理

**特徴**:
- RTE依存（I/O処理を行う）
- 状態管理（必要な場合、例：Steering）
- テスト困難（Mock RTE が必要）

### Model層（Pure Function Layer）

**責務**:
- 計算ロジック（数学・物理）を実行
- 入力から出力を生成

**特徴**:
- RTE非依存（純粋な計算）
- 副作用なし
- 容易にテスト可能（入出力のみ）
- 再利用可能

---

## 3. 各コンポーネントの責務

### Engine制御

```mermaid
flowchart LR
    Throttle["throttle<br/>0..1"]
    EStop["estop<br/>bool"]
    Logic{"estop?"}
    Result0["accel = 0.0"]
    Result1["accel = clamp<br/>× max_accel"]
    Output["drive_accel_cmd<br/>m/s²"]
    
    Throttle --> Logic
    EStop --> Logic
    Logic -->|true| Result0
    Logic -->|false| Result1
    Result0 --> Output
    Result1 --> Output
```

**Model Function**: `ComputeDriveAccel(throttle, estop, params)`

### Brake制御

```mermaid
flowchart LR
    Brake["brake<br/>0..1"]
    EStop["estop<br/>bool"]
    Logic{"estop?"}
    Result0["decel = max_decel"]
    Result1["decel = clamp<br/>× max_decel"]
    Output["brake_decel_cmd<br/>m/s²"]
    
    Brake --> Logic
    EStop --> Logic
    Logic -->|true| Result0
    Logic -->|false| Result1
    Result0 --> Output
    Result1 --> Output
```

**Model Function**: `ComputeBrakeDecel(brake, estop, params)`

### Steering制御

```mermaid
flowchart LR
    Steer["steer<br/>-1..1"]
    EStop["estop<br/>bool"]
    CalcTarget{"estop?"}
    Target0["target = 0.0"]
    Target1["target = steer ×<br/>max_steer_angle"]
    StepDyn["StepSteeringDynamics<br/>α = 1-exp<br/>-dt/τ<br/>new = curr + α×Δ"]
    Output["steer_angle_cmd<br/>rad"]
    
    Steer --> CalcTarget
    EStop --> CalcTarget
    CalcTarget -->|true| Target0
    CalcTarget -->|false| Target1
    Target0 --> StepDyn
    Target1 --> StepDyn
    StepDyn --> Output
```

**Model Function**: `StepSteeringDynamics(current, target, tau, dt, params)`

### Vehicle Dynamics

```mermaid
flowchart TD
    Input["Inputs:<br/>drive_accel, brake_decel<br/>steer_angle, estop"]
    State["Current State:<br/>v, x, y, yaw, yaw_rate"]
    
    subgraph Long["Longitudinal Step"]
        CalcAccel["accel = drive - brake<br/>- drag"]
        EStopCheck{"estop?"}
        ApplyEStop["accel -= estop_decel"]
        ClampV["v = clamp<br/>v + accel×dt"]
    end
    
    subgraph Lat["Lateral Step"]
        CalcYawRate["yaw_rate = v/L × tan(δ)"]
        UpdateYaw["yaw += yaw_rate × dt"]
        UpdatePos["x += v×cos(yaw)×dt<br/>y += v×sin(yaw)×dt"]
    end
    
    Output["New State:<br/>v', x', y', yaw', yaw_rate'"]
    
    Input --> CalcAccel
    State --> CalcAccel
    CalcAccel --> EStopCheck
    EStopCheck -->|true| ApplyEStop
    EStopCheck -->|false| ClampV
    ApplyEStop --> ClampV
    ClampV --> CalcYawRate
    CalcYawRate --> UpdateYaw
    UpdateYaw --> UpdatePos
    UpdatePos --> Output
```

**Model Functions**:
- `StepLongitudinal(state, dt, drive, brake, estop, params)`
- `StepLateral(state, dt, steer_angle, params)`

---

## 4. テスト戦略

### Model層テスト

```mermaid
flowchart LR
    Input["INPUT<br/>Deterministic"]
    Func["Pure Function<br/>No side effects"]
    Output["OUTPUT<br/>Verifiable<br/>No RTE dependency"]
    
    Input --> Func
    Func --> Output
```

**例**:
```cpp
TEST_CASE("Engine: throttle 0.5 → accel 1.0") {
    Model::EngineParams p{.max_accel_mps2 = 2.0f};
    float result = Model::ComputeDriveAccel(0.5f, false, p);
    REQUIRE(result == Approx(1.0f));
}
```

### SWC層テスト

```mermaid
flowchart LR
    Setup["Mock RTE Setup<br/>Set Input Values"]
    Call["SWC Main()<br/>Call Model Functions"]
    Verify["Output Verification<br/>Check RTE Output"]
    
    Setup --> Call
    Call --> Verify
```

**例**:
```cpp
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

## 5. 実行フロー

### 周期的実行（10ms）

```mermaid
flowchart TD
    TimeBase["TimeBase<br/>10ms tick"]
    
    subgraph Loop["Periodic Execution"]
        DriverInputSWC["DriverInputSWC::Main10ms()<br/>Read driver input → Write to RTE"]
        EngineSWC["EngineSWC::Main10ms()<br/>Read DriverInput + Safety<br/>Call Model::ComputeDriveAccel<br/>Write ActuatorCmd"]
        BrakeSWC["BrakeSWC::Main10ms()<br/>Read DriverInput + Safety<br/>Call Model::ComputeDecel<br/>Write ActuatorCmd"]
        SteeringSWC["SteeringSWC::Main10ms()<br/>Read DriverInput + Safety<br/>Call Model::StepSteeringDynamics<br/>Write ActuatorCmd"]
        VehicleDynSWC["VehicleDynamicsSWC::Main10ms()<br/>Read ActuatorCmd + VehicleState<br/>Call Model::Step Longitudinal/Lateral<br/>Write VehicleState"]
        Logging["Logging::Main10ms()<br/>Read RTE signals → CSV output"]
        Diag["Diag::Main10ms()<br/>Signal sanity check"]
    end
    
    TimeBase --> DriverInputSWC
    DriverInputSWC --> EngineSWC
    EngineSWC --> BrakeSWC
    BrakeSWC --> SteeringSWC
    SteeringSWC --> VehicleDynSWC
    VehicleDynSWC --> Logging
    Logging --> Diag
    Diag -->|Wait 10ms| TimeBase
```

---

## 6. データフロー

```mermaid
flowchart LR
    Driver["Driver Input<br/>throttle, brake, steer"]
    Engine["EngineSWC<br/>drive_accel_cmd"]
    Brake["BrakeSWC<br/>brake_decel_cmd"]
    Steering["SteeringSWC<br/>steer_angle_cmd"]
    VehicleDyn["VehicleDynamicsSWC<br/>VehicleState update"]
    Actuator["Actuation System<br/>Physical Vehicle"]
    Logging["Logging<br/>CSV Output"]
    Safety["Safety Monitor<br/>Heartbeat Check"]
    
    Driver -->|throttle| Engine
    Driver -->|brake| Brake
    Driver -->|steer| Steering
    
    Engine -->|drive_accel_cmd| VehicleDyn
    Brake -->|brake_decel_cmd| VehicleDyn
    Steering -->|steer_angle_cmd| VehicleDyn
    
    VehicleDyn -->|v, x, y, yaw...| Actuator
    VehicleDyn -->|VehicleState| Logging
    VehicleDyn -->|VehicleState| Safety
    
    Actuator -.->|Feedback| VehicleDyn
    
    style Driver fill:#ffffcc
    style Engine fill:#ccffcc
    style Brake fill:#ccffcc
    style Steering fill:#ccffcc
    style VehicleDyn fill:#ccccff
    style Actuator fill:#ffcccc
```

---

## 7. クラス・構造体定義

### RTE Signal Structures

```cpp
// Driver Input
struct DriverInput {
    float throttle;      // 0..1
    float brake;         // 0..1
    float steer;         // -1..1
};

// Actuator Command
struct ActuatorCmd {
    float drive_accel_cmd;    // m/s²
    float brake_decel_cmd;    // m/s²
    float steer_angle_cmd;    // rad
};

// Vehicle State
struct VehicleState {
    float t;              // time (s)
    float v;              // velocity (m/s)
    float x, y;           // position (m)
    float yaw;            // yaw angle (rad)
    float yaw_rate;       // yaw rate (rad/s)
    float wheel_omega;    // wheel angular velocity (rad/s)
};

// Safety Status
struct Safety {
    bool estop;
    enum SystemState { Normal, Degraded, EStop } system_state;
};
```

### Model層 パラメータ構造体

```cpp
// Engine
struct EngineParams {
    float max_accel_mps2 = 2.0f;
};

// Brake
struct BrakeParams {
    float max_decel_mps2 = 4.0f;
    float estop_max_decel_mps2 = 4.0f;
};

// Steering
struct SteeringParams {
    float max_steer_angle_rad = 0.40f;
    float time_constant_s = 0.15f;
};

// VehicleDynamics
struct VehicleParams {
    float wheel_radius_m = 0.03f;
    float wheelbase_m = 0.20f;
    float linear_drag = 0.0f;
    float max_speed_mps = 3.0f;
    float estop_decel_mps2 = 6.0f;
};
```
