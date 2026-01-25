# Engine SWC 設計書

## 1. 概要

**Engine SWC** は、ドライバー入力（throttle）を受け取り、駆動加速度指令を生成するコンポーネントです。

- **責務**: throttle (0..1) → drive_accel_cmd (m/s²)
- **周期**: 10ms
- **階層**: Model層 + SWC層の2層構造

---

## 2. アーキテクチャ

### 2.1 層の責任分離

```
┌─────────────────────────────────────────┐
│ SWC層（EngineSWC）                       │
│ - RTE から throttle, estop を読む         │
│ - Model::ComputeDriveAccel() を呼ぶ     │
│ - 結果を RTE へ書く                      │
│ - テスト対象：「正しく呼び出しているか」  │
└─────────────────────────────────────────┘
          ↓ (入出力)
┌─────────────────────────────────────────┐
│ Model層（engine_model.h）                │
│ - 純粋関数：ComputeDriveAccel()          │
│ - RTE非依存                             │
│ - テスト対象：「計算結果が正しいか」      │
└─────────────────────────────────────────┘
```

### 2.2 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | `throttle` | float (0..1) | ドライバーの加速指示（正規化値） |
| | `estop` | bool | 緊急停止フラグ |
| **パラメータ** | `max_accel_mps2` | float | 最大加速度（デフォルト: 2.0 m/s²） |
| **出力** | `drive_accel_cmd` | float (m/s²) | 駆動加速度指令 |

---

## 3. 計算ロジック（Model層）

### 3.1 ComputeDriveAccel() 関数仕様

```cpp
float ComputeDriveAccel(
    float throttle_0_1,        // 入力：0..1
    bool estop,                // 入力：緊急停止フラグ
    const EngineParams& p      // パラメータ
)
```

**計算式**：

```
if (estop):
    drive_accel_cmd = 0.0 m/s²
else:
    throttle_clamped = clamp(throttle_0_1, 0.0, 1.0)
    drive_accel_cmd = throttle_clamped × p.max_accel_mps2
```

### 3.2 要件

- ✅ throttle は 0.0～1.0 の範囲に **クランプ**
- ✅ estop = true のとき、常に 0.0 を返す
- ✅ 副作用なし（pure function）
- ✅ RTE 非依存

---

## 4. SWC層仕様

### 4.1 EngineSWC::Main10ms()

**責務**：
1. RTE から `DriverInput` を読む
2. RTE から `Safety` を読む（estop 判定用）
3. Model::ComputeDriveAccel() を呼ぶ
4. 結果を RTE の `ActuatorCmd.drive_accel_cmd` へ書く

**実装パターン**：

```cpp
void Main10ms(double dt) {
    // 1. 入力読み取り
    auto driver_in = Rte::Rte_Read_DriverInput();
    auto safety = Rte::Rte_Read_Safety();
    
    // 2. estop フラグを統合
    bool estop = safety.estop || 
                 (safety.system_state == Rte::SystemState::EStop);
    
    // 3. Model 関数を呼ぶ
    float accel = Model::ComputeDriveAccel(
        driver_in.throttle,
        estop,
        g_params
    );
    
    // 4. 出力を RTE へ書く
    auto cmd = Rte::Rte_Read_ActuatorCmd();
    cmd.drive_accel_cmd = accel;
    Rte::Rte_Write_ActuatorCmd(cmd);
}
```

---

## 5. テスト戦略

### 5.1 Model層テスト（test_engine_model.cpp）

**テストケース**：
1. `throttle=0, estop=false` → `accel=0`
2. `throttle=0.5, estop=false` → `accel=1.0`
3. `throttle=1.0, estop=false` → `accel=2.0` (clamp)
4. `throttle > 1.0, estop=false` → `accel=2.0` (clamp)
5. `throttle < 0.0, estop=false` → `accel=0` (clamp)
6. `estop=true` → `accel=0` (常に)

**ステータス**: ✅ **既に完成・PASS**

### 5.2 SWC層テスト（test_engine_swc.cpp）

**テストケース**：
1. Mock RTE に `DriverInput.throttle=0.5, Safety.estop=false` をセット
2. `EngineSWC::Main10ms()` を実行
3. `ActuatorCmd.drive_accel_cmd=1.0` になっているか確認

4. Mock RTE に `estop=true` をセット
5. 実行後、`drive_accel_cmd=0` になっているか確認

6. throttle のクランプテスト（1.5 → 2.0）

**ステータス**: ✅ **完了** (2026-01-25)

**実装ファイル**:
- Model層: [src/model/engine_model.hpp](../../src/model/engine_model.hpp)
- SWC層: [src/swc/engine_swc.cpp](../../src/swc/engine_swc.cpp)
- Mock RTE: [tests/mock_rte.h](../../tests/mock_rte.h)
- Model テスト: [tests/test_engine_model.cpp](../../tests/test_engine_model.cpp)
- SWC テスト: [tests/test_engine_swc.cpp](../../tests/test_engine_swc.cpp)

---

## 6. 動作確認

### 6.1 リグレッションテスト

- [x] Model層テストが PASS（4 test cases, 8 assertions）
- [x] SWC層テストが PASS（6 test cases, 14 assertions）
- [x] 既存シミュレーション実行で **ログ出力が変わらない** ことを確認

**シミュレーション結果 (2026-01-25)**:
```
drive_accel_cmd 統計:
  平均: 0.360 m/s²
  最大: 1.200 m/s²
  サンプル数: 1000

全体統計:
  平均throttle: 0.180
  平均速度: 0.975 m/s
  シミュレーション時間: 10.0 s
```

ベースラインログ: `build/logs/baseline_20260125_102855.csv`

---

## 7. 完了条件

- [x] Model層（engine_model.h）実装・テスト完了
- [x] SWC層 (engine_swc.cpp) Model呼び出しに置き換え
- [x] Mock RTE 実装
- [x] SWC層テスト実装
- [x] 全テスト PASS
- [x] リグレッション確認

---

## 8. 次のステップ

Engine SWC 実装完了後、以下のコンポーネントに展開可能：

- **Brake SWC**: [tasks/brake/README.md](../../tasks/brake/README.md) - ブレーキ制御実装
- **Steering SWC**: [tasks/steering/README.md](../../tasks/steering/README.md) - ステアリング制御実装
- **Safety SWC**: [tasks/safety/README.md](../../tasks/safety/README.md) - 安全監視機能実装
- **VehicleDynamics SWC**: [tasks/vehicledynamics/README.md](../../tasks/vehicledynamics/README.md) - 車両運動モデル改善

同様のテスト駆動開発パターン（Model層→Mock RTE→SWC層テスト）を適用可能。
