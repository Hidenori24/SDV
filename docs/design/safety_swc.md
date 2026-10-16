# Safety SWC 設計書

## 1. 概要

**Safety SWC** は、システムの安全状態を監視し、ハートビート監視と E-Stop 制御を行うコンポーネントです。

- **責務**: ハートビート監視、E-Stop フラグ管理、システム状態の遷移
- **周期**: 100ms（低速監視タスク）
- **重要性**: 安全機能（機能安全に対応可能な設計）

---

## 2. アーキテクチャ

### 2.1 責務分離

```
┌──────────────────────────────────────────┐
│ SWC層（SafetySupervisorSWC）             │
│ - ハートビート監視ロジック               │
│ - システム状態の遷移管理                 │
│ - E-Stop フラグの生成・伝播              │
│ - テスト対象：「状態遷移が正しいか」      │
└──────────────────────────────────────────┘
```

---

## 3. 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | 生存カウンタ | `Bsw::Diag::GetAliveCount(Component)` | 各SWCが10msごとに `ReportAlive()` で加算 |
| | `Safety.estop` | bool | 外部 E-Stop 要求（UI/ハードウェア） |
| **パラメータ** | `heartbeat_timeout_ms` | uint32_t | 生存カウンタが進まない時間の許容値（1000 ms） |
| **出力** | `Safety.estop` | bool | 緊急停止フラグ（外部要求 OR ウォッチドッグのラッチ） |
| | `Safety.system_state` | enum | Normal / Degraded / EStop |

---

## 4. ロジック

### 4.1 ハートビート監視（実装済み）

監視対象: Engine, Brake, Steering, VehicleDynamics。各SWCは `Main10ms()` の最後で `Bsw::Diag::ReportAlive()` を呼ぶ。
Safety（100ms周期）は各カウンタを前回値と比較する（`Model::UpdateHeartbeat`）。

```
カウンタが進んだ        → 無応答時間 = 0
カウンタが進まない      → 無応答時間 += 100ms
無応答時間 > timeout    → 「SWC応答なし」（既定: 1000 ms 超 = 11周期目）
```

### 4.2 状態遷移（`Model::ComputeSafetyState`）

```
応答なし 0個   → Normal
応答なし 1個   → Degraded   （SWCが復帰すれば Normal に戻る）
応答なし 2個以上 → EStop     （ラッチ。SWCが復帰しても解除されない）
```

### 4.3 E-Stop フラグ

```
estop = 外部 E-Stop 要求 OR ウォッチドッグがラッチした EStop
system_state = estop ? EStop : (Normal | Degraded)
```

- ウォッチドッグ由来の EStop は `Safety::Init()`（リセット）でのみ解除される。
- 外部要求のみの場合は、要求を取り下げれば Normal に戻る。

### 4.4 故障注入

`sdv_sim --stall <engine|brake|steering|vehicledynamics>:<t_s>` を複数指定可能。指定時刻以降そのSWCの実行を止める。
例: `sdv_sim --stall steering:3 --stall brake:3.5` → 4.12 s に Degraded、4.62 s に EStop。

---

## 5. テスト

- `tests/test_safety_logic.cpp`: Model層（状態判定、タイムアウト境界、復帰、パラメータ）
- `tests/test_safety_swc.cpp`: SWC層（ラッチ、復帰、外部E-Stop）
- `tests/test_watchdog_integration.cpp`: 実SWC+実RTEでの結合（誤検出なし60s、Degraded、EStopで停車）

---

## 6. 完了条件

- [x] ハートビート監視ロジック実装
- [x] 状態遷移ロジック実装
- [x] Model層テスト実装
- [x] SWC層テスト実装
- [x] 全テスト PASS
- [x] リグレッション確認（通常シナリオのログが従来と一致）

## 7. 今後

- Rust 版 (`rust/sdv-core`) へのウォッチドッグ移植とブラウザGUIからの故障注入
