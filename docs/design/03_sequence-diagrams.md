# Sequence Diagrams & Control Flow

## 1. Engine制御 シーケンス図（1ステップ）

```
TimeBase (10ms)
    │
    └─→ EngineSWC::Main10ms()
            │
            ├─→ RTE_Read_DriverInput()
            │   └─ return: {throttle=0.5}
            │
            ├─→ RTE_Read_Safety()
            │   └─ return: {estop=false, system_state=Normal}
            │
            ├─→ RTE_Read_ActuatorCmd()
            │   └─ return: {drive_accel_cmd=?, ...}
            │
            ├─→ Model::ComputeDriveAccel(0.5, false, params)
            │   │
            │   ├─ if (estop) return 0.0
            │   │   → false, skip
            │   │
            │   ├─ clamp(0.5) → 0.5
            │   │
            │   └─ return 0.5 × 2.0 = 1.0 m/s²
            │
            ├─ cmd.drive_accel_cmd = 1.0
            │
            └─→ RTE_Write_ActuatorCmd(cmd)
                └─ ActuatorCmd updated in RTE buffer
```

---

## 2. Steering制御 シーケンス図（複数ステップ）

```
Step 0: steer=1.0 (turn right), current_angle=0
    │
    └─→ SteeringSWC::Main10ms()
            │
            ├─→ RTE_Read_DriverInput()
            │   └─ return: {steer=1.0}
            │
            ├─→ Target = 1.0 × 0.40 rad = 0.40 rad
            │   (max_steer_angle = 0.40)
            │
            ├─→ Model::StepSteeringDynamics(
            │   current=0.0,
            │   target=0.40,
            │   tau=0.15,
            │   dt=0.01)
            │   │
            │   ├─ α = 1 - exp(-0.01/0.15) = 0.0649
            │   │
            │   └─ new_angle = 0.0 + 0.0649 × (0.40 - 0.0)
            │      = 0.0260 rad
            │
            ├─ g_steer_angle_rad = 0.0260
            │
            └─→ RTE_Write_ActuatorCmd({steer_angle_cmd=0.0260})

Step 1: steer=1.0 (still turning), current_angle=0.0260
    │
    └─→ SteeringSWC::Main10ms()
            │
            ├─→ Model::StepSteeringDynamics(
            │   current=0.0260,
            │   target=0.40,
            │   tau=0.15,
            │   dt=0.01)
            │   │
            │   └─ new_angle = 0.0260 + 0.0649 × (0.40 - 0.0260)
            │      = 0.0512 rad
            │
            └─→ RTE_Write_ActuatorCmd({steer_angle_cmd=0.0512})

... (時間とともに exponential convergence で 0.40 rad へ接近)
```

**グラフ表現**:
```
steer_angle_cmd (rad)
      0.40 ┤─────────────────────  (target)
           │
      0.30 ┤       ╱╱╱╱╱
           │   ╱╱╱╱
      0.20 ┤╱╱╱
           │
      0.10 ┤
           │
      0.00 └────────────────────────── time
           0    0.1   0.2   0.3  ...
           
      時定数 τ = 0.15s
      dt = 0.01s （サンプリング周期）
```

---

## 3. VehicleDynamics シーケンス図（複数ステップ）

```
Step 0: Initial state
    v=0, x=0, y=0, yaw=0
    Input: drive=1.0 m/s², brake=0, steer=0

Step 1: VehicleDynamicsSWC::Main10ms()
    │
    ├─→ RTE_Read_ActuatorCmd()
    │   └─ {drive_accel_cmd=1.0, brake_decel_cmd=0, steer_angle_cmd=0}
    │
    ├─→ RTE_Read_VehicleState()
    │   └─ {v=0, x=0, y=0, yaw=0, ...}
    │
    ├─→ Model::StepLongitudinal(state, dt=0.01, ...)
    │   │
    │   ├─ accel = 1.0 - 0 - 0 = 1.0 m/s²
    │   │
    │   ├─ v_next = clamp(0 + 1.0×0.01, 0, 3.0)
    │   │         = 0.01 m/s
    │   │
    │   └─ return state{v=0.01, ...}
    │
    ├─→ Model::StepLateral(state, dt=0.01, steer=0, ...)
    │   │
    │   ├─ yaw_rate = 0.01 / 0.20 × tan(0) = 0 rad/s
    │   │
    │   ├─ yaw_next = 0 + 0 × 0.01 = 0
    │   │
    │   ├─ x_next = 0 + 0.01 × cos(0) × 0.01 = 0.0001 m
    │   │
    │   ├─ y_next = 0 + 0.01 × sin(0) × 0.01 = 0 m
    │   │
    │   └─ return state{v=0.01, x=0.0001, y=0, yaw=0, ...}
    │
    └─→ RTE_Write_VehicleState(state)

Step 2: (steer=0.2 rad introduced)
    │
    ├─→ Model::StepLongitudinal(state{v=0.01}, ...)
    │   └─ v_next = 0.02 m/s
    │
    ├─→ Model::StepLateral(state{v=0.02, yaw=0}, steer=0.2)
    │   │
    │   ├─ yaw_rate = 0.02 / 0.20 × tan(0.2)
    │   │           = 0.1 × 0.2027 = 0.0203 rad/s
    │   │
    │   ├─ yaw_next = 0 + 0.0203 × 0.01 = 0.000203 rad
    │   │
    │   └─ (position updates with rotation)
    │
    └─→ RTE_Write_VehicleState(state{v=0.02, yaw=0.000203, ...})

... (時間とともに curvilinear motion)
```

**軌跡のイメージ**:
```
y (m)
  │
1 │           ╱╱╱╱
  │      ╱╱╱╱╱
  │  ╱╱╱╱
0 └──────────────── x (m)
  0   1   2   3   4

Initial straight motion, then curve due to steer
```

---

## 4. 全体フロー（1周期 = 10ms）

```
10ms TimeBase Interrupt
      │
      ├─────────────────────────────────────────┐
      │                                         │
      v                                         │
DriverInputSWC::Main10ms()                     │ Parallel
  │ read driver input (UI/scenario)            │ or
  └─ write DriverInput to RTE                  │ Sequential
                                               │
      v                                         │
EngineSWC::Main10ms()                          │
  ├─ read DriverInput, Safety                  │
  ├─ compute drive_accel                       │
  └─ write ActuatorCmd                         │
                                               │
      v                                         │
BrakeSWC::Main10ms()                           │
  ├─ read DriverInput, Safety                  │
  ├─ compute brake_decel                       │
  └─ write ActuatorCmd                         │
                                               │
      v                                         │
SteeringSWC::Main10ms()                        │
  ├─ read DriverInput, Safety                  │
  ├─ compute steer_angle (with dynamics)       │
  └─ write ActuatorCmd                         │
                                               │
      v                                         │
VehicleDynamicsSWC::Main10ms()                 │
  ├─ read ActuatorCmd                          │
  ├─ read VehicleState                         │
  ├─ integrate longitudinal                    │
  ├─ integrate lateral                         │
  └─ write VehicleState                        │
                                               │
      v                                         │
Logging::Main10ms()                            │
  ├─ read all RTE signals                      │
  └─ write CSV log                             │
                                               │
      v                                         │
Diag::Main10ms()                               │
  ├─ sanity check on signals                   │
  └─ update health status                      │
                                               │
      └─────────────────────────────────────────┘
                    ↓
           Return from interrupt
           Wait 10ms for next tick
```

---

## 5. E-Stop 発動時 シーケンス

```
Normal State
    Safety.estop = false
    Safety.system_state = Normal

E-Stop Signal (external or internal)
    │
    ├─→ Safety.estop = true
    │
    └─→ Engine/Brake/Steering SWCs detect estop flag
            │
            ├─→ EngineSWC
            │   └─ if (estop) accel_cmd = 0
            │
            ├─→ BrakeSWC
            │   └─ if (estop) decel_cmd = MAX (4.0 m/s²)
            │
            └─→ SteeringSWC
                └─ if (estop) target_angle = 0 (center steering)

Result
    Vehicle stops: drive_accel_cmd=0, brake_decel_cmd=4.0 m/s²
    Steering centers: steer_angle_cmd gradually → 0
```

---

## 6. テスト実行フロー

```
unittest execution
    │
    ├─→ Mock_Rte_Initialize()
    │   └─ Reset all global signal buffers
    │
    ├─→ TEST_CASE setup
    │   │
    │   ├─ Mock_Rte_SetDriverInput({throttle=0.5, ...})
    │   │
    │   └─ Mock_Rte_SetSafety({estop=false, ...})
    │
    ├─→ Swc::Engine::Main10ms(0.01)
    │   │
    │   ├─ RTE_Read_DriverInput() → returns mock value
    │   │
    │   ├─ RTE_Read_Safety() → returns mock value
    │   │
    │   ├─ Model::ComputeDriveAccel(...) → pure calculation
    │   │
    │   └─ RTE_Write_ActuatorCmd(cmd) → writes to mock buffer
    │
    ├─→ REQUIRE assertion
    │   │
    │   └─ cmd = Mock_Rte_Written_ActuatorCmd()
    │       REQUIRE(cmd.drive_accel_cmd == Approx(1.0f))
    │
    └─→ TEST_CASE teardown
        └─ Mock cleanup
```
