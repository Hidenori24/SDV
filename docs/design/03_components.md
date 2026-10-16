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
- C++ 実装は全 SWC で 1 つの RTE を共有。Rust 実装は **ECU ごとに 1 つの RTE** を持ち、ECU 間は仮想バスで運ぶ（[07_ecu_architecture.md](07_ecu_architecture.md)）

## ECU への割り当て（Rust 実装、ADR-0002 案A）

| ノード | 載せる SWC |
|---|---|
| VCU / ゲートウェイ | DriverInputSWC、SafetySupervisorSWC |
| Engine ECU | EngineSWC |
| Brake ECU | BrakeSWC |
| Steering ECU | SteeringSWC |
| Plant（ECU ではない） | VehicleDynamicsSWC |

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

実装状況: **実装済み**。各ノードの生存カウンタが 1000 ms（Config で変更可）を超えて進まなければ「応答なし」。
1 つで Degraded、2 つ以上で EStop（ラッチ、リセットで解除）。外部 E-Stop 要求は取り下げで Normal に戻る。詳細は [safety_swc.md](safety_swc.md)。

---

## BSW

### TimeBase
- 10ms/20ms/100msタスクを駆動
- dt供給（v1は固定dtでも可）

### Com
- C++ 実装: プロセス内（共有 RTE）
- Rust 実装: **仮想バス**（`bus.rs`）。フレームは周期送信、受信側は最新値を保持。遅延・欠損・断を注入できる。Plant 宛ては直結
- 将来: UDP Publish/Subscribe へ拡張

### Nvm(Config)
- **Rust 実装済み**（`config.rs` / `nvm.rs`）。Config は JSON（欠けた項目は既定値、未知・範囲外は拒否）
- NVM は Active / Backup の 2 面 + 世代番号 + CRC32。更新は検証に通ったものだけ保存し、再起動で有効。壊れたら Backup、両方だめなら既定値。ロールバック可能
- 詳細は [ADR-0003](../adr/ADR-0003-config-nvm.md)

### Diag+Watchdog
- 生存監視: 各 SWC（C++）／各ノードの Alive フレーム（Rust）の生存カウンタ
- system_stateの管理（Normal/Degraded/EStop）
- Rust 実装では、受信側の ECU もフェイルセーフを持つ（`failsafe.rs`）: DriverInput 途絶 → 入力 0、Safety 途絶 → 自律的に EStop 相当

### Logging
- 最小: CSV（t, inputs, cmds, state, estop）
- 目標: リプレイ（同一入力で同じ結果になる）
