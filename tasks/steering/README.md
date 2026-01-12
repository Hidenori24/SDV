# Task: Steering SWC 実装タスク一覧

## フェーズ分け

```
Phase 1: Model層実装・テスト
    → Task-1: SteeringModel (steering_model.h) を実装
    → Task-2: Model層テスト (test_steering_model.cpp) を実装
    → Task-3: 全テスト PASS確認

Phase 2: SWC層リファクタリング
    → Task-4: SteeringSWC をModel呼び出しに置き換え
    → Task-5: SWC層テスト (test_steering_swc.cpp) を実装
    → Task-6: 全テスト PASS確認

Phase 3: 統合確認
    → Task-7: リグレッション確認
```

---

## タスク詳細

### Task-1: SteeringModel (steering_model.h) を実装

**ドキュメント参照**: [docs/design/steering_swc.md#3](../../docs/design/steering_swc.md#3-計算ロジックmodel層)

**ファイル**: `src/model/steering_model.h`

**実装内容**:
```cpp
namespace Model {

struct SteeringParams {
    float max_steer_angle_rad = 0.40f;
    float time_constant_s = 0.15f;
};

inline float StepSteeringDynamics(
    float current_angle,
    float target_angle,
    float time_constant_s,
    float dt_s,
    const SteeringParams& p)
{
    // 目標舵角をクランプ
    float max_angle = p.max_steer_angle_rad;
    float target = std::clamp(target_angle, -max_angle, max_angle);
    
    // 一次遅れ: α = 1 - exp(-dt/τ)
    float alpha = 1.0f - std::exp(-dt_s / time_constant_s);
    
    // 次の舵角
    float new_angle = current_angle + alpha * (target - current_angle);
    
    return new_angle;
}

}
```

**完了条件**:
- [ ] ファイルが作成される
- [ ] コンパイルエラーなし

---

### Task-2: Model層テスト (test_steering_model.cpp) を実装

**ドキュメント参照**: [docs/design/steering_swc.md#5.1](../../docs/design/steering_swc.md#51-model層テスト)

**ファイル**: `tests/test_steering_model.cpp`

**テストケース**:
1. `current=0, target=0.4` → 段階的に 0.4 へ接近
2. `current=0.4, target=0` → 段階的に 0 へ接近
3. `target > max_angle` → clamp確認
4. 一次遅れの時定数確認（dt=τ時に63%程度到達）
5. `dt=0` → 変化なし

**完了条件**:
- [ ] コンパイルエラーなし
- [ ] 5つ以上のテストケースが実装される

---

### Task-3: 全テスト PASS確認

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
cmake --build .
ctest --verbose
```

**完了条件**:
- [ ] `test_steering_model` PASS
- [ ] ビルドエラーなし

---

### Task-4: SteeringSWC をModel呼び出しに置き換え

**ドキュメント参照**: [docs/design/steering_swc.md#4](../../docs/design/steering_swc.md#4-swc層仕様)

**ファイル**: `src/swc/steering_swc.cpp`

**修正内容**:
1. `#include "model/steering_model.h"` を追加
2. 状態変数 `g_steer_angle_rad` を保持
3. `Model::StepSteeringDynamics()` を毎ステップ呼び出す

**例**:
```cpp
static Model::SteeringParams g_params{};
static float g_steer_angle_rad = 0.0f;

void Main10ms(double dt) {
    auto in = Rte::Rte_Read_DriverInput();
    auto sf = Rte::Rte_Read_Safety();
    
    // 目標舵角（estop時は0）
    float target = sf.estop ? 0.0f : in.steer * g_params.max_steer_angle_rad;
    
    // 状態遷移
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

**完了条件**:
- [ ] コンパイルエラーなし

---

### Task-5: SWC層テスト (test_steering_swc.cpp) を実装

**ドキュメント参照**: [docs/design/steering_swc.md#5.2](../../docs/design/steering_swc.md#52-swc層テスト)

**ファイル**: `tests/test_steering_swc.cpp`

**テストケース**:
1. `steer=1.0` → 複数ステップで段階的に舵角が増加
2. `steer=-1.0` → 反対方向
3. `estop=true` → 目標舵角が 0 に強制
4. 状態遷移の正確性確認

**完了条件**:
- [ ] コンパイルエラーなし
- [ ] 4つ以上のテストケースが実装される

---

### Task-6: 全テスト PASS確認

**内容**:
```bash
ctest --verbose
```

**チェックリスト**:
- [ ] `test_steering_model` PASS
- [ ] `test_steering_swc` PASS

---

### Task-7: リグレッション確認

**内容**:
1. 既存シミュレーション実行
2. ログ出力に `steer_angle_cmd` がある
3. 舵角の時間推移が合理的（一次遅れ挙動）

**完了条件**:
- [ ] シミュレーション成功
- [ ] ログ確認完了

---

## 進捗管理

| Task | Status | 開始日 | 完了日 |
|------|--------|--------|--------|
| Task-1 | ⏳ 未開始 | - | - |
| Task-2 | ⏳ 未開始 | - | - |
| Task-3 | ⏳ 未開始 | - | - |
| Task-4 | ⏳ 未開始 | - | - |
| Task-5 | ⏳ 未開始 | - | - |
| Task-6 | ⏳ 未開始 | - | - |
| Task-7 | ⏳ 未開始 | - | - |
