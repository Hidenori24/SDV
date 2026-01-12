# Task: Safety SWC 実装タスク一覧

## フェーズ分け

```
Phase 1: Model層実装・テスト（優先度：低）
    → Task-1: SafetyLogic (safety_logic.h) を実装
    → Task-2: Model層テスト (test_safety_logic.cpp) を実装
    → Task-3: 全テスト PASS確認

Phase 2: SWC層実装
    → Task-4: SafetySupervisorSWC をLogic呼び出しに置き換え
    → Task-5: SWC層テスト (test_safety_swc.cpp) を実装
    → Task-6: 全テスト PASS確認

Phase 3: 統合確認
    → Task-7: リグレッション確認

**優先度**: 低（Engine / Brake / Steering 完了後に実装予定）
```

---

## タスク詳細

### Task-1: SafetyLogic (safety_logic.h) を実装

**ドキュメント参照**: [docs/design/safety_swc.md#4](../../docs/design/safety_swc.md#4-ロジック)

**ファイル**: `src/model/safety_logic.h`

**実装内容**:
```cpp
namespace Model {

enum class SafetyState {
    Normal = 0,
    Degraded = 1,
    EStop = 2
};

struct SafetyParams {
    uint32_t heartbeat_timeout_ms = 1000;
};

struct HeartbeatStatus {
    bool engine_ok;
    bool brake_ok;
    bool steering_ok;
    bool vehicledynamics_ok;
};

SafetyState ComputeSafetyState(const HeartbeatStatus& hb) {
    int num_failed = 0;
    if (!hb.engine_ok) num_failed++;
    if (!hb.brake_ok) num_failed++;
    if (!hb.steering_ok) num_failed++;
    if (!hb.vehicledynamics_ok) num_failed++;
    
    if (num_failed >= 2) return SafetyState::EStop;
    if (num_failed >= 1) return SafetyState::Degraded;
    return SafetyState::Normal;
}

}
```

**完了条件**:
- [ ] ファイルが作成される
- [ ] コンパイルエラーなし

---

### Task-2: Model層テスト (test_safety_logic.cpp) を実装

**ドキュメント参照**: [docs/design/safety_swc.md#5.1](../../docs/design/safety_swc.md#51-model層テスト)

**ファイル**: `tests/test_safety_logic.cpp`

**テストケース**:
1. すべてのハートビート正常 → `SafetyState::Normal`
2. 1つのSWCが異常 → `SafetyState::Degraded`
3. 2つ以上のSWCが異常 → `SafetyState::EStop`
4. 異なる異常パターン（Engine + Brake など）

**完了条件**:
- [ ] コンパイルエラーなし
- [ ] 4つ以上のテストケースが実装される

---

### Task-3: 全テスト PASS確認

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
cmake --build .
ctest --verbose
```

**完了条件**:
- [ ] `test_safety_logic` PASS
- [ ] ビルドエラーなし

---

### Task-4: SafetySupervisorSWC をLogic呼び出しに置き換え

**ドキュメント参照**: [docs/design/safety_swc.md#3](../../docs/design/safety_swc.md#3-入出力仕様)

**ファイル**: `src/swc/safety_swc.cpp`

**修正内容**:
1. `#include "model/safety_logic.h"` を追加
2. ハートビート監視ロジックを実装
   - 各SWCの最終更新時刻を RTE 経由で監視
   - 前フレームとの時刻差でハートビート判定
3. `Model::ComputeSafetyState()` を呼ぶ
4. E-Stop フラグを生成

**完了条件**:
- [ ] コンパイルエラーなし

---

### Task-5: SWC層テスト (test_safety_swc.cpp) を実装

**ドキュメント参照**: [docs/design/safety_swc.md#5.2](../../docs/design/safety_swc.md#52-swc層テスト)

**ファイル**: `tests/test_safety_swc.cpp`

**テストケース**:
1. 初期状態で `estop=false, system_state=Normal`
2. 1つのSWCのハートビートが途絶 → `system_state=Degraded`
3. 複数SWCのハートビート途絶 → `system_state=EStop`
4. 外部 E-Stop 信号で即座に `estop=true`

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
- [ ] `test_safety_logic` PASS
- [ ] `test_safety_swc` PASS

---

### Task-7: リグレッション確認

**内容**:
1. 既存シミュレーション実行
2. ハートビート監視が正常に動作
3. E-Stop フラグが適切に伝播

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

---

## 注記

- **優先度**: Engine / Brake / Steering が完了してから着手推奨
- **依存関係**: Engine / Brake / Steering / VehicleDynamics のハートビート監視が必要
