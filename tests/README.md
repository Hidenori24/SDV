# tests

C++ 単体テスト（Catch2）。`ctest` で実行します。新しい振る舞いは先に失敗するテストをここへ追加してから実装します（TDD）。

| ファイル | 対象 |
|---|---|
| `test_engine_model.cpp` / `test_engine_swc.cpp` | Engine のモデルと SWC |
| `test_brake_model.cpp` / `test_brake_swc.cpp` | Brake のモデルと SWC |
| `test_steering_model.cpp` / `test_steering_swc.cpp` | Steering のモデルと SWC（実 RTE で検証） |
| `test_vehicle_dynamics_model.cpp` / `test_vehicledynamics_swc.cpp` | 車両モデル（縦・横、タイヤの限界 `μ·g`、諸元・導出値）と SWC（実 RTE で検証） |
| `test_safety_logic.cpp` / `test_safety_swc.cpp` | Watchdog の判定ロジックと SWC（ラッチ・復帰・外部 E-Stop） |
| `test_watchdog_integration.cpp` | 実 SWC + 実 RTE の結合（誤検出なし・Degraded・EStop で停車） |

Rust 実装のテストは `rust/sdv-core/tests/`（`cargo test`）にあり、C++ の基準ログとの全行一致も確認します。

詳細な手順とPRのOK条件は [CONTRIBUTING.md](../CONTRIBUTING.md) を参照。

## GUI の E2E
ブラウザで GUI を動かす少数のスモークテストは [../e2e/README.md](../e2e/README.md)。

## 結合テスト
シミュレータ全体を通す「シナリオ + 期待値」のテストは [integration/README.md](integration/README.md)（ビルド済みの WASM を Node で動かす。CI の `integration` ジョブ）。
