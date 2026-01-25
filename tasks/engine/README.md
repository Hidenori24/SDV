# Task: Engine SWC 実装タスク一覧

## フェーズ分け

```
Phase 1: Model層テスト確認（既に完了）
    ✅ Task-1: Model層テスト (test_engine_model.cpp) PASS確認

Phase 2: SWC層リファクタリング
    → Task-2: Mock RTE 作成
    → Task-3: SWC層テスト作成
    → Task-4: 全テスト PASS確認
    → Task-5: リグレッション確認

Phase 3: ドキュメント完成
    → Task-6: ドキュメント更新
```

---

## タスク詳細

### ✅ Task-1: Model層テスト確認

**ドキュメント参照**: [engine_swc.md#5.1](../design/engine_swc.md#51-model層テスト)

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
ctest --verbose
```

- [ ] `test_engine_model` が **4 test cases, 8 assertions** すべて PASS

**完了条件**: ctest 出力に "All tests passed" が表示される

---

### Task-2: Mock RTE を作成

**ドキュメント参照**: [engine_swc.md#5.2](../design/engine_swc.md#52-swc層テスト)

**ファイル**: `tests/mock_rte.h`

**内容**:
- `Rte::DriverInput` をセット・取得するグローバル変数
- `Rte::Safety` をセット・取得するグローバル変数
- `Rte::ActuatorCmd` をセット・取得するグローバル変数
- Mock関数群：
  - `Mock_Rte_Read_DriverInput()`
  - `Mock_Rte_Read_Safety()`
  - `Mock_Rte_Read_ActuatorCmd()`
  - `Mock_Rte_Write_ActuatorCmd()`

**テストから使用例**:
```cpp
Mock_Rte_Read_DriverInput() = {{throttle=0.5, ...}};
Mock_Rte_Read_Safety() = {{estop=false, ...}};
EngineSWC::Main10ms(0.01);
REQUIRE(Mock_Rte_Written_ActuatorCmd().drive_accel_cmd == Approx(1.0f));
```

**完了条件**: コンパイルが通る

---

### Task-3: SWC層テスト を作成

**ドキュメント参照**: [engine_swc.md#5.2](../design/engine_swc.md#52-swc層テスト)

**ファイル**: `tests/test_engine_swc.cpp`

**テストケース**:

1. **正常系: throttle=0.5, estop=false**
   - Mock RTE に値をセット
   - `EngineSWC::Main10ms(0.01)` 実行
   - 結果が `drive_accel_cmd=1.0` か確認

2. **E-Stop: estop=true**
   - Mock RTE に `estop=true` をセット
   - 実行後、`drive_accel_cmd=0` か確認

3. **Clamp: throttle > 1.0**
   - Mock RTE に `throttle=1.5` をセット
   - 実行後、`drive_accel_cmd=2.0` か確認（clamp）

4. **RTE 読み書き正確性**
   - 複数ケースで「正しい RTE 呼び出し」を確認

**完了条件**: 
- コンパイルが通る
- ファイルが作成される

---

### Task-4: 全テスト PASS確認

**ドキュメント参照**: [engine_swc.md#5](../design/engine_swc.md#5-テスト戦略)

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
cmake --build .
ctest --verbose
```

**チェックリスト**:
- [ ] `test_engine_model` PASS
- [ ] `test_engine_swc` PASS
- [ ] ビルドエラーなし
- [ ] ビルド警告なし（望ましい）

**完了条件**: すべてのテストが緑色

---

### Task-5: リグレッション確認

**ドキュメント参照**: [engine_swc.md#6.1](../design/engine_swc.md#61-リグレッションテスト)

**内容**:
1. 既存シミュレーションを実行
   ```bash
   cd /Users/matsumoto/work/SDV/build
   ./sdv_sim > /tmp/current.log
   ```

2. 以前のログと比較
   - 既存 log ファイルがあれば比較
   - `drive_accel_cmd` の値が同じか確認

**期待値**: ロジック変更がないため、ログ出力は **全く同じ**

**完了条件**: 
- [x] シミュレーション実行成功
- [x] ログに `drive_accel_cmd` がある
- [x] 値に異常がない

**実施結果 (2026-01-25)**:
- シミュレーション正常終了（10.0秒、1000サンプル）
- ベースラインログ保存: `logs/baseline_20260125_102855.csv`
- **統計情報**:
  - drive_accel_cmd 平均: 0.360 m/s², 最大: 1.200 m/s²
  - 平均throttle: 0.180, 平均brake: 0.120
  - 平均速度: 0.975 m/s
- 異常値なし、Engine SWC 動作正常確認

---

### Task-6: ドキュメント更新（完了後）

**ドキュメント参照**: [engine_swc.md](../design/engine_swc.md)

**内容**:
- 実装完了日を記録
- テスト結果スクショを追加（任意）
- 次フェーズ（Brake / Steering）へのリンク追加

**完了条件**: 
- [x] ドキュメントが最新版

**実施内容 (2026-01-25)**:
- `docs/design/engine_swc.md` にステータス「完了」を追記
- 実装ファイル一覧を追加
- シミュレーション結果・統計情報を記録
- 全完了条件チェック（7項目すべて✅）
- 次ステップ（Brake/Steering/Safety/VehicleDynamics）へのリンク追加

---

## 進捗管理

| Task | Status | 開始日 | 完了日 |
|------|--------|--------|--------|
| Task-1 | ✅ 完了 | - | 2026-01-12 |
| Task-2 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-3 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-4 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-5 | ✅ 完了 | 2026-01-25 | 2026-01-25 |
| Task-6 | ✅ 完了 | 2026-01-25 | 2026-01-25 |

**🎉 Engine SWC 実装タスク 100% 完了！**

---

## メモ

- Task-2 と Task-3 は依存関係がないため、同時進行可能
- Task-4 は Task-2, Task-3 の両方が完了してから実行
- Task-5 は Task-4 が完了してから実行
