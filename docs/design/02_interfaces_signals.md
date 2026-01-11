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
