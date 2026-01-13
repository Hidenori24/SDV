# Sequence Diagrams & Control Flow (Mermaid版)

## 1. Engine制御 シーケンス図（1ステップ）

```mermaid
sequenceDiagram
    participant TB as TimeBase
    participant ES as EngineSWC
    participant RTE as RTE
    participant M as Model::Engine
    
    TB->>ES: Main10ms()
    
    ES->>RTE: RTE_Read_DriverInput()
    RTE-->>ES: {throttle=0.5}
    
    ES->>RTE: RTE_Read_Safety()
    RTE-->>ES: {estop=false, system_state=Normal}
    
    ES->>RTE: RTE_Read_ActuatorCmd()
    RTE-->>ES: {drive_accel_cmd=?}
    
    ES->>M: ComputeDriveAccel(0.5, false, params)
    Note over M: if (estop) → false, skip<br/>clamp(0.5) → 0.5<br/>return 0.5 × 2.0 = 1.0
    M-->>ES: 1.0 m/s²
    
    ES->>RTE: RTE_Write_ActuatorCmd({drive_accel_cmd=1.0, ...})
    RTE-->>TB: Updated
```

---

## 2. Steering制御 シーケンス図（複数ステップ）

```mermaid
sequenceDiagram
    participant TB as TimeBase
    participant SS as SteeringSWC
    participant RTE as RTE
    participant M as Model::Steering
    participant ST as State<br/>g_steer_angle
    
    Note over SS,ST: Step 0: steer=1.0, current_angle=0
    
    TB->>SS: Main10ms()
    SS->>RTE: RTE_Read_DriverInput()
    RTE-->>SS: {steer=1.0}
    
    Note over SS: target = 1.0 × 0.40 = 0.40 rad
    
    SS->>M: StepSteeringDynamics(0.0, 0.40, 0.15, 0.01)
    Note over M: α = 1 - exp(-0.01/0.15) = 0.0649<br/>new_angle = 0.0 + 0.0649 × 0.40<br/>         = 0.0260 rad
    M-->>SS: 0.0260 rad
    
    SS->>ST: g_steer_angle = 0.0260
    SS->>RTE: RTE_Write_ActuatorCmd({steer_angle_cmd=0.0260})
    
    Note over SS,ST: Step 1: steer=1.0, current_angle=0.0260
    
    TB->>SS: Main10ms()
    SS->>RTE: RTE_Read_DriverInput()
    RTE-->>SS: {steer=1.0}
    
    SS->>M: StepSteeringDynamics(0.0260, 0.40, 0.15, 0.01)
    Note over M: new_angle = 0.0260 + 0.0649 × (0.40-0.0260)<br/>         = 0.0512 rad
    M-->>SS: 0.0512 rad
    
    SS->>ST: g_steer_angle = 0.0512
    SS->>RTE: RTE_Write_ActuatorCmd({steer_angle_cmd=0.0512})
    
    Note over SS: Time converges exponentially to 0.40 rad<br/>Time constant τ = 0.15s, dt = 0.01s
```

---

## 3. VehicleDynamics シーケンス図（複数ステップ）

```mermaid
sequenceDiagram
    participant VD as VehicleDynSWC
    participant RTE as RTE
    participant M_L as Model::StepLongitudinal
    participant M_Lat as Model::StepLateral
    
    Note over VD: Initial: v=0, x=0, y=0, yaw=0
    Note over VD: Input: drive=1.0 m/s², brake=0, steer=0
    
    VD->>RTE: RTE_Read_ActuatorCmd()
    RTE-->>VD: {drive_accel=1.0, brake_decel=0, steer=0}
    
    VD->>RTE: RTE_Read_VehicleState()
    RTE-->>VD: {v=0, x=0, y=0, yaw=0}
    
    VD->>M_L: StepLongitudinal(state, 0.01, 1.0, 0, false)
    Note over M_L: accel = 1.0 - 0 - 0 = 1.0 m/s²<br/>v_next = clamp(0 + 1.0×0.01, 0, 3.0)<br/>       = 0.01 m/s
    M_L-->>VD: state{v=0.01}
    
    VD->>M_Lat: StepLateral(state, 0.01, steer=0)
    Note over M_Lat: yaw_rate = 0.01 / 0.20 × tan(0) = 0<br/>yaw_next = 0 + 0 × 0.01 = 0<br/>x_next = 0 + 0.01 × cos(0) × 0.01 = 0.0001 m<br/>y_next = 0 + 0.01 × sin(0) × 0.01 = 0
    M_Lat-->>VD: state{v=0.01, x=0.0001, y=0, yaw=0}
    
    VD->>RTE: RTE_Write_VehicleState(state)
    
    Note over VD: Step 2: steer=0.2 rad
    
    VD->>M_L: StepLongitudinal(state{v=0.01}, ...)
    M_L-->>VD: state{v=0.02}
    
    VD->>M_Lat: StepLateral(state{v=0.02, yaw=0}, steer=0.2)
    Note over M_Lat: yaw_rate = 0.02 / 0.20 × tan(0.2)<br/>        = 0.1 × 0.2027 = 0.0203 rad/s<br/>yaw_next = 0 + 0.0203 × 0.01 = 0.000203 rad
    M_Lat-->>VD: state{v=0.02, yaw=0.000203, ...}
    
    VD->>RTE: RTE_Write_VehicleState(state)
    
    Note over VD: Time evolves with curvilinear motion
```

---

## 4. E-Stop 発動時 シーケンス

```mermaid
sequenceDiagram
    participant Ext as External Signal
    participant Safety as Safety System
    participant RTE as RTE
    participant ES as EngineSWC
    participant BS as BrakeSWC
    participant SS as SteeringSWC
    
    Note over Ext,SS: Normal State: Safety.estop=false
    
    Ext->>Safety: E-Stop Trigger
    Safety->>RTE: Update Safety.estop = true
    
    Note over RTE: Next 10ms cycle
    
    ES->>RTE: RTE_Read_Safety()
    RTE-->>ES: {estop=true}
    ES->>ES: estop flag detected
    ES->>RTE: Write ActuatorCmd.drive_accel_cmd = 0
    
    BS->>RTE: RTE_Read_Safety()
    RTE-->>BS: {estop=true}
    BS->>BS: estop flag detected
    BS->>RTE: Write ActuatorCmd.brake_decel_cmd = 4.0 (MAX)
    
    SS->>RTE: RTE_Read_Safety()
    RTE-->>SS: {estop=true}
    SS->>SS: estop flag detected<br/>target_angle = 0
    SS->>RTE: Write ActuatorCmd.steer_angle_cmd<br/>gradually → 0
    
    Note over RTE: Result: Drive stops (v→0),<br/>Steering centers (angle→0)
```

---

## 5. 1周期実行フロー（10ms）

```mermaid
flowchart TD
    TB["TimeBase<br/>10ms Interrupt"]
    
    subgraph cycle["Execution Cycle"]
        DI["DriverInputSWC<br/>Read hardware → RTE.DriverInput"]
        ENG["EngineSWC<br/>Read → Model → Write"]
        BRK["BrakeSWC<br/>Read → Model → Write"]
        STR["SteeringSWC<br/>Read → Model → Write"]
        VD["VehicleDynamicsSWC<br/>Read → Model → Write"]
        LOG["Logging<br/>Read RTE → CSV"]
        DG["Diag<br/>Sanity Check"]
    end
    
    TB --> DI
    DI --> ENG
    ENG --> BRK
    BRK --> STR
    STR --> VD
    VD --> LOG
    LOG --> DG
    DG -->|Wait 10ms| TB
    
    style TB fill:#ffcccc
    style cycle fill:#ccffcc
    style DI fill:#ffffcc
    style ENG fill:#cce5ff
    style BRK fill:#cce5ff
    style STR fill:#cce5ff
    style VD fill:#ccccff
    style LOG fill:#ffccff
    style DG fill:#ffffcc
```

---

## 6. テスト実行フロー

```mermaid
sequenceDiagram
    participant TEST as Test Framework
    participant MRTE as Mock RTE
    participant SWC as Engine SWC
    participant MOD as Model::Engine
    
    TEST->>MRTE: Mock_Rte_Initialize()
    MRTE-->>TEST: Ready
    
    rect rgb(200, 255, 200)
        Note over TEST: TEST_CASE Setup
        TEST->>MRTE: Mock_Rte_SetDriverInput({throttle=0.5})
        MRTE-->>TEST: OK
        TEST->>MRTE: Mock_Rte_SetSafety({estop=false})
        MRTE-->>TEST: OK
    end
    
    rect rgb(200, 220, 255)
        Note over TEST: Execution
        TEST->>SWC: Swc::Engine::Main10ms(0.01)
        SWC->>MRTE: RTE_Read_DriverInput()
        MRTE-->>SWC: {throttle=0.5}
        SWC->>MRTE: RTE_Read_Safety()
        MRTE-->>SWC: {estop=false}
        SWC->>MOD: ComputeDriveAccel(0.5, false, params)
        MOD-->>SWC: 1.0
        SWC->>MRTE: RTE_Write_ActuatorCmd({drive_accel=1.0})
        MRTE-->>TEST: OK
    end
    
    rect rgb(255, 220, 200)
        Note over TEST: Verification
        TEST->>MRTE: Mock_Rte_Written_ActuatorCmd()
        MRTE-->>TEST: {drive_accel_cmd=1.0}
        TEST->>TEST: REQUIRE(cmd.drive_accel_cmd == Approx(1.0f))
    end
    
    TEST-->>TEST: PASS ✓
```

---

## グラフ表現: Steering収束動作

```
steer_angle_cmd (rad)
  0.40 ├─────────────────  (target)
       │
  0.30 ├       ╱╱╱╱╱
       │   ╱╱╱╱
  0.20 ├╱╱╱
       │
  0.10 ├
       │
  0.00 └────────────────────── time (s)
       0    0.1   0.2   0.3  ...
       
時定数 τ = 0.15s
サンプリング周期 dt = 0.01s
```

---

## グラフ表現: VehicleDynamics軌跡

```
y (m)
  │
1 │           ╱╱╱╱
  │      ╱╱╱╱╱
  │  ╱╱╱╱
0 └──────────────── x (m)
  0   1   2   3   4

初期: 直進，その後ステア導入で曲線運動
v (m/s)
  3.0 ├─────────────────  (max_speed)
      │
  2.5 ├     ╱╱╱╱╱
      │╱╱╱╱╱╱
  2.0 ├╱╱╱╱
      │
  1.5 ├╱
      │
  0.0 └────────────────────── time (s)
      0    0.5   1.0  ...
      
加速後，最大速度で飽和
```
