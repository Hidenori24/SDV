# SDV TDD実装ロードマップ

## 概要

このドキュメントは、SDV（Sensor-based Driving Vehicle）プロジェクトの TDD（Test-Driven Development）実装ロードマップを示しています。

**目標**: Model層（純粋関数）と SWC層（RTE依存）を明確に分離し、各層を独立してテストできる構造を確立する。

---

## プロジェクト構造

### ドキュメント体系

```
docs/design/
  ├─ engine_swc.md           ← Engine 制御の設計書
  ├─ brake_swc.md            ← Brake 制御の設計書
  ├─ steering_swc.md         ← Steering 制御の設計書
  ├─ vehicledynamics_swc.md  ← 物理シミュレーションの設計書
  └─ safety_swc.md           ← 安全機能の設計書
```

### タスク管理体系

```
tasks/
  ├─ engine/
  │  ├─ README.md                      ← Engine SWC タスク一覧
  │  ├─ task-2-mock-rte.md            ← Task-2: Mock RTE 実装
  │  └─ task-3-test-engine-swc.md     ← Task-3: SWC テスト実装
  ├─ brake/
  │  └─ README.md                      ← Brake SWC タスク一覧（7 tasks）
  ├─ steering/
  │  └─ README.md                      ← Steering SWC タスク一覧（7 tasks）
  ├─ vehicledynamics/
  │  └─ README.md                      ← VehicleDynamics タスク一覧（7 tasks）
  └─ safety/
     └─ README.md                      ← Safety SWC タスク一覧（7 tasks、優先度低）
```

---

## 実装フェーズ

### Phase 1: Engine SWC（進行中）

**ステータス**: ✅ Model層実装完了、SWC層修正中

**目的**:
- Model層の純粋関数化を確認
- Mock RTE の設計を確立
- SWC層テストのパターンを確立

**タスク**:
- [x] Task-1: Model層テスト確認（既に PASS）
- [ ] Task-2: Mock RTE 作成
- [ ] Task-3: SWC層テスト作成
- [ ] Task-4~6: テスト PASS確認

**期待值**: 全テスト GREEN になると、Brake / Steering / VehicleDynamics も同じパターンで進める

---

### Phase 2: Brake SWC（後続）

**目的**: Engine SWC と同じパターンで Brake 制御を実装

**タスク**: 7 tasks （Task-1 ～ Task-7）

**期待値**: Engine SWC の成功パターンをそのまま適用

---

### Phase 3: Steering SWC（後続）

**目的**: 一次遅れダイナミクスを含む制御ロジックのテスト

**タスク**: 7 tasks （Task-1 ～ Task-7）

**特徴**:
- Model層：`StepSteeringDynamics()` 純粋関数
- SWC層：状態変数を保有し、毎ステップ更新

---

### Phase 4: VehicleDynamics SWC（後続）

**目的**: 物理シミュレーションの完全実装

**タスク**: 7 tasks （Task-1 ～ Task-7）

**特徴**:
- Model層：`StepLongitudinal()` 既実装 + `StepLateral()` 追加
- SWC層：両関数を組み合わせて状態を更新

---

### Phase 5: Safety SWC（後続・優先度低）

**目的**: ハートビート監視と E-Stop 制御の実装

**タスク**: 7 tasks （Task-1 ～ Task-7、優先度低）

**特徴**:
- Engine / Brake / Steering / VehicleDynamics 完了後に着手推奨

---

## テスト戦略

### Model層テスト

**責務**: 計算ロジックが正しいか確認

**フレームワーク**: Catch2（既に統合）

**例**:
```cpp
TEST_CASE("EngineModel: throttle scaling") {
    Model::EngineParams p{.max_accel_mps2 = 2.0f};
    REQUIRE(Model::ComputeDriveAccel(0.5f, false, p) == Approx(1.0f));
}
```

### SWC層テスト

**責務**: RTE I/O と Model呼び出しが正しいか確認

**フレームワーク**: Catch2 + Mock RTE

**例**:
```cpp
TEST_CASE("EngineSWC: RTE I/O") {
    Mock_Rte_SetDriverInput({{throttle=0.5f, ...}});
    EngineSWC::Main10ms(0.01);
    REQUIRE(Mock_Rte_Written_ActuatorCmd().drive_accel_cmd == Approx(1.0f));
}
```

### 統合テスト（リグレッション）

**責務**: 既存動作が変わらないか確認

**方法**: 既存シミュレーション実行 → ログ出力比較

---

## ファイル・フォルダ構造

### Model層（src/model/）

```
src/model/
  ├─ engine_model.h              ✅ 実装完了
  ├─ brake_model.h               ❌ 未実装
  ├─ steering_model.h            ❌ 未実装
  ├─ vehicledynamics_model.h     ✅ 部分実装（StepLongitudinal のみ）
  └─ safety_logic.h              ❌ 未実装
```

### SWC層（src/swc/）

```
src/swc/
  ├─ engine_swc.cpp              🔄 修正中（Model呼び出し化）
  ├─ brake_swc.cpp               ❌ 未修正
  ├─ steering_swc.cpp            ❌ 未修正
  ├─ vehicledynamics_swc.cpp     ⚠️ 部分修正（StepLateral 追加待ち）
  ├─ driverinput_swc.cpp         ✅ 変更不要
  └─ safety_swc.cpp              ❌ 未修正
```

### テスト層（tests/）

```
tests/
  ├─ test_engine_model.cpp                   ✅ 4 tests PASS
  ├─ test_vehicle_dynamics_model.cpp         ✅ 2 tests PASS
  ├─ mock_rte.h                              ❌ 未実装
  ├─ test_engine_swc.cpp                     ❌ 未実装
  ├─ test_brake_model.cpp                    ❌ 未実装
  ├─ test_brake_swc.cpp                      ❌ 未実装
  ├─ test_steering_model.cpp                 ❌ 未実装
  ├─ test_steering_swc.cpp                   ❌ 未実装
  ├─ test_vehicledynamics_swc.cpp            ❌ 未実装
  ├─ test_safety_logic.cpp                   ❌ 未実装
  └─ test_safety_swc.cpp                     ❌ 未実装
```

---

## 現在のステータス

| コンポーネント | Model層 | SWC層 | テスト | リグレッション |
|---|---|---|---|---|
| **Engine** | ✅ 完了 | 🔄 修正中 | ⏳ 実装待ち | ⏳ 確認待ち |
| **Brake** | ❌ 未実装 | ❌ 未修正 | ❌ 未実装 | ❌ 未確認 |
| **Steering** | ❌ 未実装 | ❌ 未修正 | ❌ 未実装 | ❌ 未確認 |
| **VehicleDynamics** | ⚠️ 部分完 | ⚠️ 部分修正 | ⚠️ 部分完 | ⏳ 確認待ち |
| **Safety** | ❌ 未実装 | ❌ 未修正 | ❌ 未実装 | ❌ 未確認 |

---

## 次のステップ

### 即座に実装すべき（Phase 1: Engine）

1. **[tasks/engine/task-2-mock-rte.md](../tasks/engine/task-2-mock-rte.md)** を読む
2. `tests/mock_rte.h` を実装
3. `tests/test_engine_swc.cpp` を実装
4. ビルド・テスト実行

### その後（Phase 2: Brake）

各タスクファイルに沿って実装を進める

---

## 参考資料

- [docs/design/](./design/) - 各コンポーネントの設計書
- [tasks/](../tasks/) - タスク管理・進捗管理
- CMakeLists.txt - Catch2 統合設定
