# Task: Brake SWC 実装タスク一覧

## フェーズ分け

```
Phase 1: Model層実装・テスト
    → Task-1: BrakeModel (brake_model.h) を実装
    → Task-2: Model層テスト (test_brake_model.cpp) を実装
    → Task-3: 全テスト PASS確認

Phase 2: SWC層リファクタリング
    → Task-4: BrakeSWC をModel呼び出しに置き換え
    → Task-5: SWC層テスト (test_brake_swc.cpp) を実装
    → Task-6: 全テスト PASS確認

Phase 3: 統合確認
    → Task-7: リグレッション確認
```

---

## タスク詳細

### Task-1: BrakeModel (brake_model.h) を実装

**ドキュメント参照**: [docs/design/brake_swc.md#3](../../docs/design/brake_swc.md#3-計算ロジックmodel層)

**ファイル**: `src/model/brake_model.h`

**実装内容**:
```cpp
namespace Model {

struct BrakeParams {
    float max_decel_mps2 = 4.0f;
    float estop_max_decel_mps2 = 4.0f;
};

inline float ComputeBrakeDecel(float brake_0_1, bool estop, const BrakeParams& p) {
    if (estop) return p.estop_max_decel_mps2;
    float b = std::clamp(brake_0_1, 0.0f, 1.0f);
    return b * p.max_decel_mps2;
}

}
```

**完了条件**:
- [x] ファイルが作成される
- [x] コンパイルエラーなし

---

### Task-2: Model層テスト (test_brake_model.cpp) を実装

**ドキュメント参照**: [docs/design/brake_swc.md#5.1](../../docs/design/brake_swc.md#51-model層テスト)

**ファイル**: `tests/test_brake_model.cpp`

**テストケース**:
1. `brake=0, estop=false` → `decel=0`
2. `brake=0.5, estop=false` → `decel=2.0`
3. `brake=1.0, estop=false` → `decel=4.0`
4. `estop=true` → `decel=4.0`
5. `brake > 1.0` → clamp で `decel=4.0`

**完了条件**:
- [x] コンパイルエラーなし
- [x] 5つ以上のテストケースが実装される

---

### Task-3: 全テスト PASS確認

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
cmake --build .
ctest --verbose
```

**完了条件**:
- [x] `test_brake_model` PASS
- [x] ビルドエラーなし

---

### Task-4: BrakeSWC をModel呼び出しに置き換え

**ドキュメント参照**: [docs/design/brake_swc.md#4](../../docs/design/brake_swc.md#4-swc層仕様)

**ファイル**: `src/swc/brake_swc.cpp`

**修正内容**:
1. `#include "model/brake_model.h"` を追加
2. `g_params` を `Model::BrakeParams` 型に変更
3. 計算ロジックを `Model::ComputeBrakeDecel()` に置き換え

**例**:
```cpp
auto cmd = Rte::Rte_Read_ActuatorCmd();
bool estop = sf.estop || sf.system_state == Rte::SystemState::EStop;
cmd.brake_decel_cmd = Model::ComputeBrakeDecel(in.brake, estop, g_params);
Rte::Rte_Write_ActuatorCmd(cmd);
```

**完了条件**:
- [x] コンパイルエラーなし
- [x] 既存動作と同一（リグレッション確認後）

---

### Task-5: SWC層テスト (test_brake_swc.cpp) を実装

**ドキュメント参照**: [docs/design/brake_swc.md#5.2](../../docs/design/brake_swc.md#52-swc層テスト)

**ファイル**: `tests/test_brake_swc.cpp`

**テストケース**:
1. `brake=0.5, estop=false` → Model呼び出しが正しい
2. `estop=true` → 強制制動
3. `brake > 1.0` → clamp確認
4. RTE Read/Write の正確性

**完了条件**:
- [x] コンパイルエラーなし
- [x] 4つ以上のテストケースが実装される

---

### Task-6: 全テスト PASS確認

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
ctest --verbose
```

**チェックリスト**:
- [x] `test_brake_model` PASS
- [x] `test_brake_swc` PASS

---

### Task-7: リグレッション確認

**内容**:
1. 既存シミュレーション実行
2. ログ出力に `brake_decel_cmd` がある
3. 値に異常がない

**完了条件**:
- [x] シミュレーション成功
- [x] ログ確認完了

---

## 進捗管理

| Task | Status | 開始日 | 完了日 |
|------|--------|--------|--------|
| Task-1 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-2 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-3 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-4 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-5 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-6 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-7 | ✅ 完了 | 2026-01-25 | 2026-01-25 |

**🎉 Brake SWC 実装タスク 100% 完了！**

**テスト結果**:
- Model層テスト: 8 test cases, 9 assertions - すべて PASS
- SWC層テスト: 6 test cases - すべて PASS
- 全体: 24 test cases, 29 assertions - すべて PASS

**シミュレーション結果**:
- brake_decel_cmd 平均: 0.480 m/s², 最大: 2.400 m/s²
- サンプル数: 1000（10秒間）
- 異常値なし、Brake SWC 動作正常確認
