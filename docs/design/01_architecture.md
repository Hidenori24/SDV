# SDV システムアーキテクチャ

> **このドキュメントの範囲**: C++ 実装（`src/`、1 プロセス・1 つの RTE）の構成。
> Rust 実装（`rust/sdv-core`）は ECU ごとに RTE を持ち、仮想バスでつながる構成に進んでいる。現行の全体像は
> [07_ecu_architecture.md](07_ecu_architecture.md) を参照（経緯は [ADR-0002](../adr/ADR-0002-ecu-partitioning.md)）。

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
        estop (bool)

Logic:
  if (estop):
    decel = max_decel_mps2
  else:
    decel = clamp(brake) × max_decel_mps2

Model Function: ComputeBrakeDecel(brake, estop, params)
```

### Steering制御

```
Input:  steer (-1..1)      Output: steer_angle_cmd (rad)
        estop (bool)
        dt (s)

State: steer_angle_rad (current steering angle)

Logic:
  target = steer × max_steer_angle (if !estop)
  target = 0.0 (if estop)
  
  α = 1 - exp(-dt / τ)
  new_angle = current + α × (target - current)

Model Function: StepSteeringDynamics(current, target, tau, dt, params)
```

### Vehicle Dynamics

```
Input:  drive_accel_cmd (m/s²)
        brake_decel_cmd (m/s²)
        steer_angle_cmd (rad)
        estop (bool)

State:  v, x, y, yaw, yaw_rate, wheel_omega

Logic (Bicycle Model):
  Longitudinal:
    accel = drive - brake - drag
    if (estop): accel -= estop_decel
    v = clamp(v + accel×dt, 0, v_max)
    
  Lateral:
    yaw_rate = v / L × tan(δ)
    yaw += yaw_rate × dt
    x += v×cos(yaw)×dt
    y += v×sin(yaw)×dt

Model Functions:
  StepLongitudinal(state, dt, drive, brake, estop, params)
  StepLateral(state, dt, steer_angle, params)
```

---

## 4. テスト戦略

### Model層テスト

```
Pure Function Test (Catch2)

INPUT → [Pure Function] → OUTPUT
↓                          ↓
Deterministic         Verifiable
No side effects       No RTE dependency
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

```
Integration Test with Mock RTE (Catch2 + mock_rte.h)

Mock RTE Setup → SWC Main() → Output Verification
    ↓              ↓              ↓
Set Input      Call Model    Check RTE
Values         Functions     Output
```

**例**:
```cpp
TEST_CASE("EngineSWC: reads throttle, writes accel") {
    Mock_Rte_SetDriverInput({{throttle=0.5f}});
    Mock_Rte_SetSafety({{estop=false}});
    
    Swc::Engine::Main10ms(0.01);
    
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == Approx(1.0f));
}
```

---

## 5. 実行フロー

### 周期的実行（C++ の `main.cpp` のタスク順序）

```
TimeBase（10ms ティック）
  ├─ 10ms タスク（毎ティック）
  │    1. EngineSWC::Main10ms()          ─ Model::ComputeDriveAccel()
  │    2. BrakeSWC::Main10ms()           ─ Model::ComputeBrakeDecel()
  │    3. SteeringSWC::Main10ms()        ─ Model::StepSteeringDynamics()
  │    4. VehicleDynamicsSWC::Step10ms() ─ 縦・横の更新 → VehicleState
  │    5. Logging::Tick10ms()            ─ RTE の値を CSV へ
  ├─ 20ms タスク（偶数ティック）
  │    DriverInputSWC::Main20ms()        ─ シナリオから DriverInput を更新
  └─ 100ms タスク（10 ティックごと）
       SafetySupervisorSWC::Main100ms()  ─ 各 SWC の生存カウンタを監視 → Normal/Degraded/EStop
```

各 SWC（Engine / Brake / Steering / VehicleDynamics）は実行の最後に `Bsw::Diag::ReportAlive()` で生存を報告する。
Safety はその生存カウンタが進んでいるかを 100ms ごとに確認する（詳細は [safety_swc.md](safety_swc.md)）。

---

## 6. データフロー

```
Driver Input
    ↓
┌───┴─────────────────────────┐
│                             │
v                             v
EngineSWC                 SteeringSWC
  ↓                          ↓
drive_accel_cmd         steer_angle_cmd
  ↓                          ↓
  └─────────┬────────────────┘
            ↓
      BrakeSWC
            ↓
      brake_decel_cmd
            ↓
      ┌──────┴──────────────┐
      │                     │
      v                     v
VehicleDynamicsSWC    [Actuation System]
  ↓
[Updated State]
  v, x, y, yaw, ...
  ↓
  ├─→ Logging (CSV output)
  └─→ Safety Monitor (Heartbeat check)
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

  - SafetySupervisorSWC
- RTE
  - Rte_Read / Rte_Write（Signal Buffer）
  - Runnable dispatcher（タスク周期で呼び出す）
- BSW
  - TimeBase（dt、周期タスク）
  - Com（信号の外部入出力：v1はプロセス内、後でUDP）
  - Nvm(Config)（パラメータ永続化）
  - Diag/Watchdog（状態監視、ハートビート）
  - Logging（CSV、リプレイ）
- Platform
  - OS依存（時計、ファイル、ソケット等）抽象

## データフロー（v1）

```mermaid
flowchart LR
  UI[Python UI/Scenario] -->|DriverInput| Com
  Com --> RTE
  RTE --> DriverInputSWC
  DriverInputSWC -->|signals| RTE

  RTE --> EngineSWC
  RTE --> BrakeSWC
  RTE --> SteeringSWC

  EngineSWC -->|DriveCmd| RTE
  BrakeSWC -->|BrakeCmd| RTE
  SteeringSWC -->|SteerAngle| RTE

  RTE --> VehicleDynamicsSWC
  VehicleDynamicsSWC -->|VehicleState| RTE
  RTE --> Logging
  RTE --> Safety[SafetySupervisorSWC]
  Safety -->|E-Stop| RTE
```

## 実行形態

- C++ 実装: 単一プロセス（シンプルな周期スケジューラで Runnable を呼ぶ）
- Rust 実装: ECU ごとの RTE + 仮想バス（[07_ecu_architecture.md](07_ecu_architecture.md)）。WASM にして GitHub Pages のブラウザ GUI で動かす
- 将来: Com の UDP 化、SWC 差し替え（Control 系など）のプロセス分離

## 状態遷移（Safety）

- Normal: 通常動作
- Degraded: 一部異常（ログは継続、制限動作）
- EStop: 強制停止（出力無効、速度0へ収束）

EStopは「最優先」で、他のSWCの出力より常に上位で適用される。

## 物理モデル（v1）

v1は “それっぽさ” を優先し、最小の2Dモデルを採用する。

- 縦（速度）: エンジン駆動 + ブレーキ減速（飽和・一次遅れは任意）
- 横（旋回）: キネマティック・バイシクルモデル
- 車輪回転: v と半径 r から wheel_omega を算出（整合を維持）
