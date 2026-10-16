# 信号I/F定義（Signals & Interfaces）

## 信号カテゴリ

- DriverInput（UI→車両）
- ActuatorCmd（制御→プラント）
- VehicleState（プラント→全体）
- Safety（E-Stop、状態）
- System（Heartbeat、Version、Diag）

## DriverInput

- throttle: float (0.0..1.0)
- brake: float (0.0..1.0)
- steer: float (-1.0..+1.0)
- mode: enum (Manual, Demo, Replay) ※v1ではManual/Demoのみでも可

## ActuatorCmd（SWC出力）

- drive_accel_cmd: float (m/s^2) もしくは drive_force_cmd (N)
- brake_decel_cmd: float (m/s^2) （正の値で減速量）
- steer_angle_cmd: float (rad) （舵角）

v1では簡便のため、加減速は “加速度/減速度” 表現を優先する。

## VehicleState（観測値）

- t: float (s)
- x: float (m)
- y: float (m)
- yaw: float (rad)
- v: float (m/s)
- yaw_rate: float (rad/s)
- wheel_omega: float (rad/s)

## Safety/System

- estop: bool
- system_state: enum (Normal, Degraded, EStop)
- heartbeat: counter + timestamp
- sw_versions: string（各SWCのバージョン、v1は固定文字列で可）

## RTEアクセス規約

- SWCは直接他SWCの内部を参照しない
- 共有は Signal のみ
- すべてのSWCは以下のみ使用する
  - Rte_Read_<Signal>()
  - Rte_Write_<Signal>()

## ECU 間のバスフレーム（Rust 実装）

Rust 実装では、上記の信号は ECU ごとの RTE に置かれ、ECU 間は仮想バスのフレームで運ばれる（[07_ecu_architecture.md](07_ecu_architecture.md)）。

| フレーム | 送信 → 受信 | 周期 | 内容 |
|---|---|---|---|
| DriverInput | VCU → Engine / Brake / Steering | 20 ms | throttle, brake, steer |
| Safety | VCU → Engine / Brake / Steering（バス）、Plant（直結） | 10 ms | estop, system_state |
| Alive | Engine / Brake / Steering / Plant → VCU | 10 ms | 生存カウンタ |
| VehicleState | Plant → VCU | 10 ms | t, x, y, yaw, v, yaw_rate, wheel_omega |
| CmdDrive / CmdBrake / CmdSteer | Engine / Brake / Steering → Plant（直結） | 10 ms | drive_accel_cmd / brake_decel_cmd / steer_angle_cmd |

- 受信側は「最新値 + 受信からの経過時間」を持つ。DriverInput が 200 ms、Safety が 100 ms 途絶すると、ECU はフェイルセーフに入る
- 直結リンク（Plant 宛て）はバス障害（遅延・欠損・断）の影響を受けない

## 外部データ形式（JSON）

| 形式 | 役割 | 詳細 |
|---|---|---|
| Config | SWC/BSW の調整パラメータ（最大加速度・最大舵角・Watchdog のタイムアウトなど） | [ADR-0003](../adr/ADR-0003-config-nvm.md)、`rust/README.md` |
| Scenario | ドライバー入力の時系列と、時刻指定のイベント（SWC 停止・E-Stop・バス断など） | `rust/README.md` |

どちらも「欠けた項目は既定値、未知の項目と範囲外は拒否（全エラーを項目パス付きで返す）」の方針で検証する。

## 符号規約

- `steer`: **正 = 左旋回**（反時計回り、yaw 増加）。GUI のスライダー／キーは「右 = 正」で、モデルへ渡すときに符号を反転する
- `yaw`: 反時計回りが正。`yaw_rate` も同じ

