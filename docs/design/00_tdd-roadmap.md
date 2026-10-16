# SDV TDD実装ロードマップ

## 概要

SDV（Software-Defined Vehicle）の自作プロジェクトを、TDD（Test-Driven Development）で進めてきた記録と現状。
進め方・テストの実行方法・PR の OK 条件は [CONTRIBUTING.md](../../CONTRIBUTING.md) を参照。

**方針**: Model 層（純粋関数）と SWC 層（RTE 依存）を明確に分け、各層を独立してテストする。
C++ を参照実装として固め、Rust 実装（`rust/sdv-core`）が C++ の基準ログと全行一致することを確認しながら、ECU 分割・Config/NVM・シナリオへ広げた。

全体の進捗は [00_overview.md](00_overview.md)、現行の構成は [07_ecu_architecture.md](07_ecu_architecture.md)。

---

## プロジェクト構造

### ドキュメント体系

```
docs/design/
  ├─ 00_overview.md ～ 07_ecu_architecture.md   ← 全体設計・進捗・ECU 構成
  ├─ engine_swc.md / brake_swc.md / steering_swc.md
  │  vehicledynamics_swc.md / safety_swc.md      ← SWC ごとの設計書
docs/adr/
  ├─ ADR-0001 AUTOSAR-like を採用
  ├─ ADR-0002 疑似 ECU 分割
  └─ ADR-0003 Config / NVM
```

### タスク管理体系

```
tasks/
  ├─ engine/          ← 完了
  ├─ brake/           ← 完了
  ├─ steering/        ← 完了
  ├─ safety/          ← 完了（Watchdog）
  └─ vehicledynamics/ ← 完了
```

---

## 実装フェーズ（C++ 参照実装）

| Phase | 内容 | 状態 |
|---|---|---|
| 1 | Engine SWC（Mock RTE の設計、SWC 層テストのパターン確立） | ✅ 完了 |
| 2 | Brake SWC | ✅ 完了 |
| 3 | Steering SWC（一次遅れ。Model 層 + 実 RTE でのテスト） | ✅ 完了 |
| 4 | VehicleDynamics SWC（縦: `StepLongitudinal`、横: `StepLateral`、合成: `StepVehicle`） | ✅ 完了 |
| 5 | Safety SWC（Watchdog：生存カウンタの監視 → Degraded / EStop ラッチ） | ✅ 完了 |

## 実装フェーズ（Rust 実装）

| Step | 内容 | 状態 |
|---|---|---|
| 1 | Model 層・シミュレーションループの移植（C++ の基準ログと全行一致） | ✅ |
| 2 | Safety の Watchdog を移植し、故障注入に対応 | ✅ |
| 3 | RTE と SWC の分離（ECU ごとに別の RTE を持てる構造） | ✅ |
| 4 | 仮想バスと ECU 分割（ADR-0002 案A） | ✅ |
| 5 | バス障害（遅延・欠損・断）、VCU 停止、ECU のフェイルセーフ | ✅ |
| 6 | Config（JSON・検証）と疑似 NVM（A/B・CRC・ロールバック）（ADR-0003） | ✅ |
| 7 | シナリオ（入力と時刻指定イベントの JSON）、再現性 | ✅ |
| 8 | ブラウザ GUI（WASM）、GitHub Pages | ✅ |
| 9 | 更新後の自動ロールバック（試験適用） | ⏳ 未着手 |

---

## テスト戦略

### Model 層テスト
計算ロジックが正しいか（純粋関数の入出力）。Catch2（C++）/ `cargo test`（Rust）。

### SWC 層テスト
RTE の入出力と Model 呼び出しが正しいか。C++ の Engine / Brake は Mock RTE（`tests/mock_rte.h`）、
Steering / Safety は実際の RTE を使って検証する。Rust は渡された `Rte` を直接読み書きして検証する。

### 統合・再現性
- `test_watchdog_integration.cpp`: 実 SWC + 実 RTE + 実 Safety（誤検出なし・Degraded・EStop で停車）
- C++ の基準ログ（`rust/sdv-core/tests/golden/*.csv`、通常 + 故障注入 3 パターン）を CI で生成し、Rust が全行再現する
- 同じ Config + 同じシナリオなら同じログ（Rust の `tests/scenario.rs`）

---

## ファイル・フォルダ構造（C++）

### Model 層（`src/model/`）

```
src/model/
  ├─ engine_model.h              ✅
  ├─ brake_model.h               ✅
  ├─ steering_model.h            ✅
  ├─ vehicledynamics_model.h     ✅ 縦・横（StepLongitudinal / StepLateral / StepVehicle）
  └─ safety_logic.h              ✅
```

### SWC 層（`src/swc/`）

```
src/swc/
  ├─ engine_swc.cpp              ✅ Model 呼び出し
  ├─ brake_swc.cpp               ✅ Model 呼び出し
  ├─ steering_swc.cpp            ✅ Model 呼び出し
  ├─ vehicledynamics_swc.cpp     ✅ Model 呼び出し（StepVehicle）
  ├─ driverinput_swc.cpp         ✅ 内蔵シナリオ
  └─ safety_swc.cpp              ✅ Watchdog
```

### テスト層（`tests/`）

```
tests/
  ├─ mock_rte.h                              ✅
  ├─ test_engine_model.cpp / test_engine_swc.cpp                 ✅
  ├─ test_brake_model.cpp / test_brake_swc.cpp                   ✅
  ├─ test_steering_model.cpp / test_steering_swc.cpp             ✅
  ├─ test_vehicle_dynamics_model.cpp                             ✅ 縦・横
  ├─ test_vehicledynamics_swc.cpp                                ✅
  ├─ test_safety_logic.cpp / test_safety_swc.cpp                 ✅
  └─ test_watchdog_integration.cpp                               ✅
```

---

## 次のステップ

1. 更新直後に Degraded / EStop になったら自動でロールバックする試験適用（ADR-0003 の後続）
2. ログへ Config の世代・適用時刻を記録
3. ECU 構成を設定で切り替える（ADR-0002 の案B・案C）

## 参考資料

- [docs/design/](./) - 各コンポーネントの設計書
- [tasks/](../../tasks/) - タスク管理・進捗管理
- [CONTRIBUTING.md](../../CONTRIBUTING.md) - TDD の進め方、テスト、CI
