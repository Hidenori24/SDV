# VehicleDynamics SWC 設計書

## 1. 概要

**VehicleDynamics SWC** は、駆動・制動・舵角指令を受け取り、車両の物理的な状態（速度、位置、姿勢）を時間発展させるコンポーネントです。

- **責務**: (drive_accel_cmd, brake_decel_cmd, steer_angle_cmd) → (v, x, y, yaw, wheel_omega)
- **周期**: 10ms
- **モデル**: キネマティック自転車モデル（バイシクルモデル）
- **階層**: Model層 + SWC層の2層構造

---

## 2. アーキテクチャ

### 2.1 層の責任分離

```
┌──────────────────────────────────────────────┐
│ SWC層（VehicleDynamicsSWC）                  │
│ - RTE から ActuatorCmd を読む                │
│ - RTE から前フレームの VehicleState を読む   │
│ - Model::StepLongitudinal() を呼ぶ           │
│ - Model::StepLateral() を呼ぶ                │
│ - 新しい状態を RTE へ書く                    │
│ - テスト対象：「Model呼び出しの正確性」      │
└──────────────────────────────────────────────┘
          ↓ (入出力)
┌──────────────────────────────────────────────┐
│ Model層（vehicledynamics_model.h）           │
│ - 純粋関数：StepLongitudinal() (既実装)      │
│ - 純粋関数：StepLateral()                    │
│ - バイシクルモデルの物理                     │
│ - RTE非依存                                 │
│ - テスト対象：「物理計算が正しいか」          │
└──────────────────────────────────────────────┘
```

### 2.2 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | `drive_accel_cmd` | float (m/s²) | 駆動加速度 |
| | `brake_decel_cmd` | float (m/s²) | 制動減速度 |
| | `steer_angle_cmd` | float (rad) | 舵角指令 |
| | `estop` | bool | 緊急停止フラグ |
| **状態** | `t` | float (s) | シミュレーション時刻 |
| | `v` | float (m/s) | 速度（縦） |
| | `yaw` | float (rad) | ヨー角（姿勢） |
| | `yaw_rate` | float (rad/s) | ヨーレート |
| | `x`, `y` | float (m) | 位置 |
| | `wheel_omega` | float (rad/s) | 車輪回転速度 |
| **パラメータ** | `wheel_radius_m` | float | 車輪半径（0.03 m） |
| | `wheelbase_m` | float | ホイールベース（0.20 m） |
| | `max_speed_mps` | float | 最大速度（3.0 m/s） |
| | `estop_decel_mps2` | float | E-Stop時追加減速（6.0 m/s²） |

---

## 3. 計算ロジック（Model層）

### 3.1 StepLongitudinal() 関数仕様（既実装）

```cpp
VehicleState StepLongitudinal(
    const VehicleState& s,
    float dt,
    float drive_accel_cmd,
    float brake_decel_cmd,
    bool estop,
    const VehicleParams& p
)
```

**責務**：
- 速度を更新
- 車輪回転速度を更新
- 加速度：drive - brake - drag
- estop時は追加減速
- 速度は 0..max_speed にクランプ

**ステータス**: ✅ **既実装・テスト完了**

### 3.2 StepLateral() 関数仕様（未実装）

```cpp
VehicleState StepLateral(
    const VehicleState& s,
    float dt,
    float steer_angle_cmd,
    const VehicleParams& p
)
```

**責務**：
- ヨーレートを計算：yaw_rate = v / L × tan(δ)
- ヨー角を更新：yaw = yaw + yaw_rate × dt
- 位置を更新：
  - x = x + v × cos(yaw) × dt
  - y = y + v × sin(yaw) × dt

---

## 4. SWC層仕様

### 4.1 VehicleDynamicsSWC::Main10ms()

**責務**：
1. RTE から `ActuatorCmd` を読む
2. RTE から前フレームの `VehicleState` を読む
3. Model::StepLongitudinal() を呼ぶ（速度更新）
4. Model::StepLateral() を呼ぶ（位置・姿勢更新）
5. 新しい状態を RTE へ書く

---

## 5. テスト戦略

### 5.1 Model層テスト

**StepLongitudinal（既実装・PASS）**：
- ✅ 速度が正の加速度で増加
- ✅ ブレーキで速度が減らない
- ✅ E-Stop で追加減速

**StepLateral（未実装）**：
- [ ] steer_angle_cmd=0 → yaw は変わらない
- [ ] steer_angle_cmd > 0 → yaw_rate > 0（右旋回）
- [ ] v=0 のとき yaw_rate = 0（静止時は旋回なし）
- [ ] 位置が更新される（x, y）

### 5.2 SWC層テスト（test_vehicledynamics_swc.cpp）

**テストケース**：
1. 複数ステップ実行後、速度が増加
2. ブレーキ後、速度が減少
3. ハンドル切ると、yaw が変わる
4. E-Stop で強制的に減速

**ステータス**: ❌ **未実装**

---

## 6. 物理モデル（参考）

### キネマティック自転車モデル

```
前進方向：v(t+dt) = clamp(v(t) + (a_drive - a_brake - a_drag) × dt, 0, v_max)

旋回：yaw_rate = v / L × tan(δ)
      yaw(t+dt) = yaw(t) + yaw_rate × dt

位置：x(t+dt) = x(t) + v × cos(yaw) × dt
      y(t+dt) = y(t) + v × sin(yaw) × dt
```

- L: ホイールベース (0.20 m)
- δ: 舵角 (rad)
- v: 速度 (m/s)

---

## 7. 完了条件

- [x] Model層 StepLongitudinal 実装・テスト完了
- [ ] Model層 StepLateral 実装・テスト追加
- [ ] SWC層 Model呼び出しに置き換え
- [ ] SWC層テスト実装
- [ ] 全テスト PASS
- [ ] リグレッション確認
