# ADR-0003: Config / NVM（パラメータ更新とロールバック）

## Status
Accepted。**改訂1（schema 2: 諸元・物理・導出値・調整値の分離）を末尾に追記**（2026-10-16）

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

---

## 改訂1: schema 2 — 諸元・物理・導出値・調整値に分ける

### 背景
シミュレータのステアリングが実車に比べて敏感すぎることが分かった。原因の一つは、旋回の速さが速度に比例して増え続け、
**タイヤの限界（横加速度の上限）がない**こと。3 m/s でフルロックにすると 363°/s（約 1.9 G）で曲がる。
この上限は手で入れる数値ではなく、**路面の摩擦（`μ × g`）から決まる値**である。
そこで、設定を「車そのもの」「世界」「そこから計算される値」「ソフトの設定」に分ける。

### 4 つの区分
| 区分 | 中身 | 誰が使う | 入力元 |
|---|---|---|---|
| `vehicle_spec`（諸元） | 車の寸法・性能 | シミュレータの Plant と、実機の ECU ソフト | カタログ・実測 |
| `physics`（物理） | 世界が決める値（重力、摩擦、走行抵抗） | **シミュレータの Plant だけ**（実機では自然が決める） | 路面の種類で選ぶ |
| 導出値（`derived`、**読み取り専用**） | 上の 2 つから計算される値 | 画面に表示して実車と比べる。Plant の飽和 | 計算（設定には書かない） |
| `tuning`（調整） | 制御側の設定（E-Stop 時の減速、Watchdog、フェイルセーフ） | 実機の ECU ソフト | 調整 |

### JSON（schema 2）
```json
{
  "schema": 2,
  "vehicle_spec": {
    "wheelbase_m": 0.20, "wheel_radius_m": 0.03,
    "max_steer_angle_rad": 0.40, "steer_time_constant_s": 0.15,
    "top_speed_mps": 3.0, "max_accel_mps2": 2.0, "max_decel_mps2": 4.0, "estop_decel_mps2": 6.0
  },
  "physics": { "gravity_mps2": 9.81, "tire_friction_mu": 0.6, "drag_coeff": 0.15 },
  "tuning": {
    "estop_max_decel_mps2": 4.0,
    "safety": { "heartbeat_timeout_ms": 1000 },
    "failsafe": { "driver_timeout_ms": 200, "safety_timeout_ms": 100 }
  }
}
```
欠けた項目は既定値、未知の項目と範囲外は全体を拒否（全エラーをパス付きで返す）。方針は schema 1 と同じ。

### schema 1 との対応と互換
| schema 1 | schema 2 |
|---|---|
| `engine.max_accel_mps2` | `vehicle_spec.max_accel_mps2` |
| `brake.max_decel_mps2` | `vehicle_spec.max_decel_mps2` |
| `brake.estop_max_decel_mps2` | `tuning.estop_max_decel_mps2` |
| `steering.max_steer_angle_rad` / `time_constant_s` | `vehicle_spec.max_steer_angle_rad` / `steer_time_constant_s` |
| `vehicle.wheel_radius_m` / `wheelbase_m` | `vehicle_spec.wheel_radius_m` / `wheelbase_m` |
| `vehicle.max_speed_mps` | `vehicle_spec.top_speed_mps` |
| `vehicle.estop_decel_mps2` | `vehicle_spec.estop_decel_mps2` |
| `vehicle.linear_drag` | `physics.drag_coeff` |
| `safety.*` / `failsafe.*` | `tuning.safety.*` / `tuning.failsafe.*` |
| （なし） | `physics.gravity_mps2`、`physics.tire_friction_mu`（新規） |

- **schema 1 は自動で schema 2 に変換して読む**。見分け方: `"schema": 1`、または `schema` がなく旧形式のキー（`engine` / `brake` / `steering` / `vehicle` / `safety` / `failsafe`）を含む場合
- 出力（`to_json`、NVM への保存）は常に schema 2。NVM に残っている schema 1 のデータは、次の保存で schema 2 になる
- 新規項目（`gravity_mps2`、`tire_friction_mu`）は、schema 1 から変換したときは既定値を使う

### 導出値（`derived`）
| 値 | 式 |
|---|---|
| 最小旋回半径 | `R_min = L / tan(δ_max)` |
| 横加速度の上限 | `a_lat_max = μ × g` |
| 摩擦が効き始める速度 | `v_crit = sqrt(a_lat_max × R_min)` |
| 速度 v でフルロックしたときの有効な最大舵角 | `δ_eff(v) = min(δ_max, atan(L × a_lat_max / v²))` |
| 速度 v でフルロックしたときの旋回の速さ | `ψ̇(v) = v / L × tan(δ_eff(v))` |

### Plant の摩擦による飽和
Plant（横方向）は、有効な舵角を `δ_eff(v)` に制限する（`|v × ψ̇| ≤ μ × g`）。式は上の導出値と同じ。
- 低速では従来と同じ（`v ≤ v_crit` で影響なし）。既定値では `v_crit ≈ 1.67 m/s`、基準ログの走行は最大 2.1 m/s²（0.21 G）なので**基準ログは変わらない**
- 3 m/s でフルロックにすると、旋回が 363°/s → 約 112°/s、半径が 0.47 m → 1.5 m になる
- 「制御側で舵角を速度に応じて絞る」機能は別（`tuning`）。物理側の飽和は、絞らなかったときに何が起きるかの模擬、制御側はそれを避ける設計

### 検証範囲（schema 2 で新規・変更するもの）
| 項目 | 範囲 | 既定 |
|---|---|---|
| `vehicle_spec.top_speed_mps` | 0.1 〜 50 | 3.0 |
| `physics.gravity_mps2` | 1.0 〜 25 | 9.81 |
| `physics.tire_friction_mu` | 0.05 〜 1.5 | 0.6 |
| `physics.drag_coeff` | 0 〜 2.0 | 0.15 |

そのほか（`wheelbase_m`、`wheel_radius_m`、`max_steer_angle_rad`、加速度・減速度、タイムアウト）は schema 1 と同じ範囲。
関係チェック: `tuning.estop_max_decel_mps2 >= vehicle_spec.max_decel_mps2`。

### 置き場所
- 式（旋回の飽和、導出値）は **C++ の Model 層**に置く（ECU ソフトも使う式の単一のソース）。Rust 側の同じ式は、境界の整理（ADR-0004 予定）が終わるまでの暫定の写し
- 設定の読み込み・検証・変換は、当面は Rust の `config.rs`（NVM と同様、後で C++ へ移す）

### 実装の進め方（小さなPRで）
1. 本改訂（ADR）
2. C++ Model: `VehicleSpec` / `Physics`、導出値、摩擦による飽和（基準ログは変わらない）
3. Rust: schema 2 の読み込み（schema 1 の自動変換を含む）と飽和。C++ の基準ログと一致
4. GUI: 「諸元」「物理」「導出値（読み取り専用）」「調整」を分けて表示・編集、路面のプリセット
5. 実機の候補（PiRacer / PiCar-X）の実測値のプリセット（実測値が揃ってから）、入力側（速度に応じた舵角の制限）の見直し
