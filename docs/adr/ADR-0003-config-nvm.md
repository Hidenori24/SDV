# ADR-0003: Config / NVM（パラメータ更新とロールバック）

## Status
Accepted（ユーザー合意済みの進め方に沿って実装する）

## Context
v1 の完了条件に「パラメータ更新（Config/NVM）で挙動が変化し、ロールバックできる」がある（`docs/design/00_overview.md`）。
現状、各SWCのパラメータ（最大加速度、最大舵角、ウォッチドッグのタイムアウトなど）はコード中の既定値で、変更も復旧もできない。

SDV（ソフトでクルマを定義する）の見せ場として、次を満たしたい。
- パラメータを更新すると挙動が変わる
- 不正な更新は拒否され、壊れたデータからも復旧できる
- 更新を元に戻せる（ロールバック）

## Decision

### 1. 対象範囲
**Config = SWC/BSW の調整パラメータ**のみ。次は Config に含めない。
- 故障注入（SWC停止・バス障害・VCU停止）: テスト条件であり、製品の設定ではない
- ドライバー入力・シナリオ: 別形式（シナリオ JSON）で外部化する（本ADRの段階5）
- バス遅延・欠損: 環境条件（故障注入側）

### 2. 形式と項目（JSON）
```json
{
  "schema": 1,
  "engine":   { "max_accel_mps2": 2.0 },
  "brake":    { "max_decel_mps2": 4.0, "estop_max_decel_mps2": 4.0 },
  "steering": { "max_steer_angle_rad": 0.40, "time_constant_s": 0.15 },
  "vehicle":  { "wheel_radius_m": 0.03, "wheelbase_m": 0.20, "linear_drag": 0.15,
                "max_speed_mps": 3.0, "estop_decel_mps2": 6.0 },
  "safety":   { "heartbeat_timeout_ms": 1000 },
  "failsafe": { "driver_timeout_ms": 200, "safety_timeout_ms": 100 }
}
```
- **欠けた項目は既定値**で補う（部分更新できる）。**未知の項目は拒否**（typo を黙って無視しない）
- 既定値は現在のコード中の定数と同一にする（既定 Config で動かすと、C++ の golden ログと一致する）
- `schema` は将来の形式変更用。未対応のバージョンは拒否する
- 依存: `serde` / `serde_json`（`deny_unknown_fields`）

### 3. 検証（更新は全部通るか、全部拒否か）
範囲を外れた値が1つでもあれば**全体を拒否**し、どの項目がなぜ不正かを一覧で返す（項目パス + 理由）。

| 項目 | 範囲 | 既定 |
|---|---|---|
| engine.max_accel_mps2 | 0 〜 10 | 2.0 |
| brake.max_decel_mps2 | 0.1 〜 10 | 4.0 |
| brake.estop_max_decel_mps2 | 0.1 〜 15（かつ `>= max_decel_mps2`） | 4.0 |
| steering.max_steer_angle_rad | 0.05 〜 0.6（tan の発散を避ける） | 0.40 |
| steering.time_constant_s | 0.01 〜 2.0 | 0.15 |
| vehicle.wheel_radius_m | 0.005 〜 1.0 | 0.03 |
| vehicle.wheelbase_m | 0.05 〜 5.0 | 0.20 |
| vehicle.linear_drag | 0 〜 2.0 | 0.15 |
| vehicle.max_speed_mps | 0.1 〜 50 | 3.0 |
| vehicle.estop_decel_mps2 | 0 〜 20 | 6.0 |
| safety.heartbeat_timeout_ms | 200 〜 10000（Safety 周期 100 ms の2倍以上） | 1000 |
| failsafe.driver_timeout_ms | 50 〜 2000 | 200 |
| failsafe.safety_timeout_ms | 30 〜 1000（バスの Safety 周期 10 ms の3倍以上） | 100 |

数値は有限値（NaN / Inf は拒否）。

### 4. NVM（疑似不揮発メモリ）
A/B 2面 + 世代番号 + CRC32 で壊れたデータから復旧する。

```
slot "active"  : 現在の Config   { seq, crc32, json }
slot "backup"  : 1つ前の Config  { seq, crc32, json }
```
- **更新**（`commit`）: 検証に通ったときだけ、`backup ← active`、`active ← 新Config（seq+1）`。検証に落ちたら何も変えない
- **起動時の読み込み**（`load`）: `active` の CRC が正しければ採用。壊れていれば `backup` へフォールバック。両方壊れていれば既定値
- **ロールバック**（`rollback`）: `active ← backup`（`backup` は空にする。2回続けては戻れない）
- 記憶装置は抽象化（`read(slot)` / `write(slot, bytes)`）。テストとWASMはメモリ上の実装、ブラウザは localStorage に保存する
- **適用タイミング**: Config は**リセット（再起動）で有効**になる。実機のコンフィグ更新が再起動を伴うことに合わせる。GUI は「保存して再起動」で適用する

### 5. 試験適用（自動ロールバック）— 後続
更新直後の一定時間（例: 5 s）に Degraded / EStop になったら、自動でロールバックして元の Config で再起動する。
OTA の「更新後ヘルスチェック → 失敗なら戻す」に相当。段階的に、手動ロールバックの後で実装する。

## 実装の進め方（小さなPRで）
1. 本ADR
2. Rust: Config 構造体、JSON の読み込み・範囲検証・既定値（既定 Config が現状の定数と一致することをテスト）。SWC / Sim が Config を使うようにする。挙動は不変（golden 一致）
3. Rust: NVM（A/B・CRC・フォールバック・ロールバック）
4. WASM API と GUI: JSON 編集 → 検証結果表示 → 保存して再起動 / ロールバック / NVM 破損の注入。localStorage に保存
5. シナリオ入力の外部化（JSON）、更新後の自動ロールバック

## Consequences
- 良い点: 挙動をコードを書き換えずに変えられる。壊れた更新・壊れた記憶領域から復旧できる。構成（ECU割り当て等）を Config 化する足場になる
- 悪い点: 依存クレート（serde）が増え、WASM が大きくなる。C++ には反映しない（Rust のみ）
- 将来: ECU 構成（SWC→ECU の割り当て）を Config に載せる。署名付き更新・差分更新は非ゴール
