# 部品設計（Components）

## SWC一覧（v1上限）

1. DriverInputSWC
2. EngineSWC
3. BrakeSWC
4. SteeringSWC
5. VehicleDynamicsSWC
6. SafetySupervisorSWC

## BSW一覧（v1上限）

- TimeBase
- Com
- Nvm(Config)
- Diag+Watchdog
- Logging

## RTE

- Signal Buffer（各信号の最新値を保持）
- SWC Runnable の呼び出し（タスク周期）

---

## 1) DriverInputSWC

責務: 外部入力（UI/シナリオ）を内部信号へ反映する。  
I/F:
- Read: ComからDriverInput取得（v1はRTE経由でも良い）
- Write: DriverInput（throttle/brake/steer/mode）

Runnable:
- DriverInputSWC_Main_20ms()

受け入れ条件:
- 値域クランプ（0..1、-1..1）が必ず適用される

---

## 2) EngineSWC（最初に作る）

責務: throttleから駆動の“意図”（加速度指令など）を生成する。  
I/F:
- Read: throttle, estop, system_state
- Write: drive_accel_cmd

Runnable:
- EngineSWC_Main_10ms()

パラメータ（Config）例:
- max_accel_mps2
- accel_tau_s（一次遅れ時定数、任意）
- max_speed_mps（速度上限はVehicleDynamics側で適用してもよい）

受け入れ条件:
- throttle=0で drive_accel_cmd が0（または0へ収束）
- estop=trueで出力は0固定

---

## 3) BrakeSWC

責務: brakeから減速度指令を生成する。  
I/F:
- Read: brake, estop
- Write: brake_decel_cmd

Runnable:
- BrakeSWC_Main_10ms()

パラメータ例:
- max_decel_mps2
- decel_tau_s（任意）

受け入れ条件:
- brake=0で 0
- estop=trueで最大減速（または別途EStopロジックで0へ収束）

---

## 4) SteeringSWC

責務: steer入力から舵角指令を生成する（サーボ遅れ・飽和）。  
I/F:
- Read: steer, estop
- Write: steer_angle_cmd

Runnable:
- SteeringSWC_Main_10ms()

パラメータ例:
- max_steer_angle_rad
- steer_tau_s

受け入れ条件:
- steer=0で舵角0へ収束
- 舵角は必ず飽和する

---

## 5) VehicleDynamicsSWC（Plant）

責務: 駆動・制動・操舵を受けて VehicleState を更新し、wheel_omega等の観測を出す。  
I/F:
- Read: drive_accel_cmd, brake_decel_cmd, steer_angle_cmd, estop
- Write: VehicleState

Runnable:
- VehicleDynamicsSWC_Step_10ms()

モデル（v1）:
- 縦: v += (drive_accel_cmd - brake_decel_cmd - resist(v)) * dt
- 横: yaw_rate = v/L * tan(steer_angle)
- Pose: x,y,yawを離散更新
- 車輪回転: wheel_omega = v/r

受け入れ条件:
- vは0未満にならない
- estop時はv→0へ収束（一定の減速で落とす）

---

## 6) SafetySupervisorSWC

責務: ハートビート監視、異常時にE-Stopへ遷移させる。  
I/F:
- Read: heartbeat（各SWC/BSW）、VehicleState（任意）
- Write: estop, system_state

Runnable:
- SafetySupervisorSWC_Main_100ms()

受け入れ条件:
- 監視対象がタイムアウトしたらEStopをTrueにする
- EStop後は明示解除が必要（v1はリセットで解除でも可）

---

## BSW

### TimeBase
- 10ms/20ms/100msタスクを駆動
- dt供給（v1は固定dtでも可）

### Com
- v1: プロセス内（Pythonはファイル/標準入出力/簡易ソケットで接続しても良い）
- v1+: UDP Publish/Subscribe へ拡張（後で）

### Nvm(Config)
- JSONやiniでOK
- 更新時はバックアップを作りロールバック可能にする（v1のSDV要件）

### Diag+Watchdog
- 生存監視（カウンタ、時刻）
- system_stateの管理（Normal/Degraded/EStop）

### Logging
- 最小: CSV（t, inputs, cmds, state, estop）
- 目標: リプレイ（同一入力で同じ結果になる）
