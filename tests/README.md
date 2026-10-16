# tests

C++ 単体テスト（Catch2）。`ctest` で実行します。新しい振る舞いは先に失敗するテストをここへ追加してから実装します（TDD）。

| ファイル | 対象 |
|---|---|
| `test_engine_model.cpp` / `test_engine_swc.cpp` | Engine のモデルと SWC |
| `test_brake_model.cpp` / `test_brake_swc.cpp` | Brake のモデルと SWC |
| `test_steering_model.cpp` / `test_steering_swc.cpp` | Steering のモデルと SWC（実 RTE で検証） |
| `test_vehicle_dynamics_model.cpp` | 車両モデル |
| `test_safety_logic.cpp` / `test_safety_swc.cpp` | Watchdog の判定ロジックと SWC（ラッチ・復帰・外部 E-Stop） |
| `test_watchdog_integration.cpp` | 実 SWC + 実 RTE の結合（誤検出なし・Degraded・EStop で停車） |

Rust 実装のテストは `rust/sdv-core/tests/`（`cargo test`）にあり、C++ の基準ログとの全行一致も確認します。

詳細な手順とPRのOK条件は [CONTRIBUTING.md](../CONTRIBUTING.md) を参照。
