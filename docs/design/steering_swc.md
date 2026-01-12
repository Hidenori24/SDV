# Steering SWC 設計書

## 1. 概要

**Steering SWC** は、ドライバー入力（steer）を受け取り、舵角指令を生成するコンポーネントです。  
**重要**: サーボダイナミクス（一次遅れ）を含む。

- **責務**: steer (-1..1) → steer_angle_cmd (rad)
- **周期**: 10ms
- **階層**: Model層 + SWC層の2層構造
- **特徴**: 状態変数（現在の舵角）を保有

---

## 2. アーキテクチャ

### 2.1 層の責任分離

```
┌──────────────────────────────────────────────┐
│ SWC層（SteeringSWC）                         │
│ - 状態変数を保有：float steer_angle_rad      │
│ - RTE から steer を読む                      │
│ - Model::StepSteeringDynamics() を呼ぶ       │
│ - 新しい状態を保存                           │
│ - 結果を RTE へ書く                          │
│ - テスト対象：「状態遷移が正しいか」          │
└──────────────────────────────────────────────┘
          ↓ (入出力 + 状態)
┌──────────────────────────────────────────────┐
│ Model層（steering_model.h）                  │
│ - 純粋関数：StepSteeringDynamics()            │
│ - 一次遅れダイナミクス                       │
│ - RTE非依存                                 │
│ - テスト対象：「遅れ計算が正しいか」          │
└──────────────────────────────────────────────┘
```

### 2.2 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | `steer` | float (-1..1) | ドライバーのハンドル入力（正規化値） |
| | `estop` | bool | 緊急停止フラグ |
| **パラメータ** | `max_steer_angle_rad` | float | 最大舵角（デフォルト: 0.40 rad） |
| | `time_constant_s` | float | 一次遅れ時定数（デフォルト: 0.15 s） |
| **状態** | `steer_angle_rad` | float | 現在の舵角 |
| **出力** | `steer_angle_cmd` | float (rad) | 舵角指令（出力） |

---

## 3. 計算ロジック（Model層）

### 3.1 StepSteeringDynamics() 関数仕様

```cpp
float StepSteeringDynamics(
    float current_angle,      // 現在の舵角 (rad)
    float target_angle,       // 目標舵角 (rad)
    float time_constant_s,    // 時定数 τ (s)
    float dt_s,               // ステップ時間 (s)
    const SteeringParams& p   // パラメータ
)
```

**計算式**（一次遅れ）：

```
target = clamp(steer × p.max_steer_angle_rad, -max, +max)
α = 1 - exp(-dt / τ)
new_angle = current_angle + α × (target - current_angle)
```

### 3.2 要件

- ✅ 一次遅れダイナミクスを実装
- ✅ `steer` は -1..1 に **クランプ**
- ✅ 目標舵角は ±`max_steer_angle_rad` で **飽和**
- ✅ estop = true のとき、目標舵角 = 0.0
- ✅ 副作用なし（pure function）

---

## 4. SWC層仕様

### 4.1 SteeringSWC::Main10ms()

**責務**：
1. RTE から `DriverInput` を読む
2. RTE から `Safety` を読む（estop 判定用）
3. 目標舵角を計算（estop 時は 0.0）
4. Model::StepSteeringDynamics() を呼ぶ
5. **新しい状態を内部変数に保存**
6. 結果を RTE の `ActuatorCmd.steer_angle_cmd` へ書く

**重要**：SWC側で状態を保有し、毎ステップ更新する

```cpp
static float g_steer_angle_rad = 0.0f;  // 状態変数

void Main10ms(double dt) {
    auto in = Rte::Rte_Read_DriverInput();
    auto sf = Rte::Rte_Read_Safety();
    
    // 目標舵角（estop時は0）
    float target = sf.estop ? 0.0f : in.steer;
    
    // Model関数で次の状態を計算
    g_steer_angle_rad = Model::StepSteeringDynamics(
        g_steer_angle_rad,
        target,
        g_params.time_constant_s,
        dt,
        g_params
    );
    
    // 出力
    auto cmd = Rte::Rte_Read_ActuatorCmd();
    cmd.steer_angle_cmd = g_steer_angle_rad;
    Rte::Rte_Write_ActuatorCmd(cmd);
}
```

---

## 5. テスト戦略

### 5.1 Model層テスト（test_steering_model.cpp）

**テストケース**：
1. `current=0, target=0.4` → 時間とともに 0 → 0.4 へ収束
2. `current=0.4, target=0` → 0.4 → 0 へ収束
3. `target > max_angle` → clamp確認
4. 一次遅れの時定数確認（dt = τ時に63%到達）

**ステータス**: ❌ **未実装**

### 5.2 SWC層テスト（test_steering_swc.cpp）

**テストケース**：
1. 初期状態で `steer=1.0` → 段階的に舵角が増加（1ステップでは完全には到達しない）
2. `steer=-1.0` → 反対方向
3. `estop=true` → 目標舵角が 0 に強制される
4. 複数ステップで「遅れ挙動」を確認

**ステータス**: ❌ **未実装**

---

## 6. 完了条件

- [ ] Model層（steering_model.h）実装・テスト完了
- [ ] SWC層 (steering_swc.cpp) Model呼び出しに置き換え
- [ ] SWC層テスト実装
- [ ] 全テスト PASS
- [ ] リグレッション確認
