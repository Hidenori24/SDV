# ADR-0005: FOTA（ECU ソフトの更新・試験適用・ロールバック）

## Status

Proposed（下書き 2026-10-17）。「要確認事項」が決まったら Accepted にして「決定事項」を追記する。

> 番号について: ADR-0003 改訂1 の「置き場所」で ADR-0004 を「C++/Rust の境界の整理」に予約しているため、本ADRは 0005 とした。入れ替える場合はファイル名とリンクを直す。

## Context

ADR-0003 で **Config（データ）の更新** は成立している（検証 → A/B 2面 + CRC32 → 再起動で有効 → ロールバック）。
一方、SDV の要点である **ソフト（コード）の更新** は未着手で、`docs/design/05_safety_logging_update.md` の「v1+：SWC差し替え」に方針メモがあるだけである。

本ADRで、ECU に載るソフトを **配信 → 検証 → 書き込み → 試験適用 → 確定 or 自動ロールバック** する仕組み（FOTA）を設計する。
ADR-0002 で作った「ECU 分割・仮想バス・故障注入・Watchdog」が、そのまま FOTA の検証環境になる。

### 前提・制約（既存の決定との整合）

- **Safety（SafetySupervisorSWC）は更新対象外**（常駐。05 の方針）。Safety が載る VCU は、当面は更新対象にしない
- **既存の Watchdog は緩めない**。Alive 途絶 > 1000 ms で、1ECU → Degraded、2ECU 以上 → EStop（ラッチ）
- **基準ログ（golden）を壊さない**。更新を行わないシナリオでは、ログは従来と全行一致する
- 記憶装置は ADR-0003 の抽象（`read(slot)` / `write(slot, bytes)`）を再利用する。テストと WASM はメモリ上、ブラウザは localStorage
- 主役は Rust。C++ には反映しない（ADR-0002 と同じ）
- バスは CAN 風（固定周期フレーム）。大きなデータの転送には向かない（→ 更新チャネルを分ける）
- ADR-0003 は「署名付き更新・差分更新は非ゴール」としている。本ADRでもまず署名は扱わない（要確認 3）

### Config 更新との違い

| 観点 | Config 更新（ADR-0003） | FOTA（本ADR） |
| ---- | ----------------------- | ------------- |
| 対象 | JSON のパラメータ | ECU 上の SWC 実装 |
| 大きさ | 小さい（1 フレームで足りない程度） | 大きい（分割転送が必要） |
| 検証 | 構文・範囲 | サイズ・ハッシュ・互換性（信号I/F、Config schema）・ダウングレード |
| 失敗の影響 | 値が不適切 | 起動しない・止まる |
| ロールバック | 手動（後続で自動） | **起動失敗・不合格・Commit なしで自動** |

## 検討した案

### イメージの実体

#### 案A: 論理イメージ（ビルド済みの複数バージョンから選ぶ）【推奨：最初に実装】

- SWC の実装を複数バージョン持っておき、manifest が「どの ECU のどの実装を、どのバージョンで有効にするか」を指す
- 転送するバイト列は実体のないペイロード（ハッシュ・サイズ・分割転送は本物）
- 配信、検証、A/B、試験適用、ロールバックといった **プロトコルと状態遷移を本物にできる**
- 既存 SWC のコードを変えず、golden も保てる。実装量は小〜中
- 弱点: 「コードが入れ替わる」実感は薄い

#### 案B: 実 WASM モジュール（ECU ごとに別モジュール）

- SWC を別の WASM にして、ECU が実行時に読み込む。ブラウザ上で本当にホットスワップできる
- ホスト側（ブラウザは WebAssembly API、`cargo test` は wasmtime など）が必要で、変更量が大きい。ADR-0004（境界の整理）と干渉しやすい

#### 案C: Config の拡張のみ

- Config にバージョンや署名を足すだけ。コードの更新にならないので、FOTA とは呼べない（不採用）

### 配信経路

- **案X: 制御バスに載せる** … 固定周期フレームの帯域を圧迫し、制御の周期（10 / 20 ms）に影響する
- **案Y: 更新チャネルを別に持つ**【推奨】 … 実車の診断/リプログラミング用チャネル（UDS over CAN-FD / DoIP）に相当する。帯域・遅延・欠損率を別に設定でき、バスの故障注入（遅延・欠損・断）も同じように効く

## Decision（案）

1. **案A → 案B の順で進める。** 段階1で仕組みを固め、段階2（実 WASM 化、署名）は別ADRで扱う
2. **配信経路は案Y（更新チャネル）。** 制御は既存のバスフレームで行い、データ本体は更新チャネルで送る
3. **更新の単位は「ECU ごとの SWC イメージ」。** 当面の対象は Engine / Brake / Steering の 3 ECU。VCU（Safety 同居）は対象外
4. **VCU に UpdateManager（更新の司令塔）を置く。** Safety SWC とは独立で、Safety の挙動を変えられない。外部（GUI、将来は Wi-Fi 経由のサーバ）からパッケージを受け取る
5. **A/B 2面 + ブート制御 + 試験適用。** 確定は「Commit が来たとき」だけ。Commit が来なければ ECU が自分で戻る
6. **更新は車両停止中に 1 ECU ずつ。** 2 ECU が同時に止まると EStop になるため
7. **Watchdog は緩めない。** ECU の再起動時間（`reboot_ms`）を `heartbeat_timeout_ms` より短く規定する（要確認 4）

## 仕様案

### パッケージと manifest

```
{
  "manifest_schema": 1,
  "image": {
    "target_ecu": "steering",
    "swc": "SteeringSWC",
    "version": "1.1.0",
    "size_bytes": 12345,
    "sha256": "（16 進 64 文字）"
  },
  "requires": {
    "signal_if_version": 1,
    "config_schema": [1, 2]
  },
  "policy": {
    "allow_downgrade": false,
    "trial_ms": 5000
  }
}
```

- 未知の項目は拒否（ADR-0003 と同じ方針）。数値は有限値のみ
- バージョンは `MAJOR.MINOR.PATCH`。現在より小さい（または同じ）バージョンは `allow_downgrade: false` なら拒否
- `signal_if_version`: 信号I/F（`02_interfaces_signals.md` とバスマトリクス）の版。**現状は版が無いので、定義する必要がある**（要確認 7）
- `config_schema`: そのイメージが読める Config の schema（現行は 1 と 2）。NVM 内の Config と合わなければ拒否

### ECU 側の状態遷移

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Receiving: Request（前提条件 OK）
    Receiving --> Verifying: 全ブロック受信
    Receiving --> Idle: Abort / タイムアウト / エラー
    Verifying --> Staged: size・sha256・互換性 OK
    Verifying --> Idle: 検証 NG（active は不変）
    Staged --> Rebooting: Activate
    Rebooting --> Trial: 新イメージで起動
    Rebooting --> RolledBack: 起動失敗（試行回数切れ）
    Trial --> Committed: Commit（ヘルスチェック合格）
    Trial --> RolledBack: 不合格 / Commit が来ない
    Committed --> Idle
    RolledBack --> Idle
```

- 受信中・検証中に失敗しても、**active は一切変わらない**（書き込み先は非稼働面だけ）
- 途中で止まった転送は、最後に ack したブロックから再開できる（Request を再送）

### NVM（ECU ごと）

```
slot "img_a" / "img_b" : { version, sha256, bytes }
slot "boot_ctl"        : { seq, crc32, active: A|B, trial: bool, trial_boots_left: n }
```

- 更新は **非稼働面に書く → 検証 → boot_ctl を `trial = true`、`active` を新しい面に切り替える**
- 起動時: `trial` かつ `trial_boots_left == 0` なら、前の面へ戻す。`trial` なら残り回数を減らして新しい面で起動
- Commit: `trial = false`。前の面は、次の更新まで **フォールバック用に残す**
- `boot_ctl` の保存は ADR-0003 と同じ A/B + CRC32 の作法。壊れていたら、検証済みの面（前の面）を採用する

### 前提条件（Request を受け付ける条件）

| 条件 | 理由 |
| ---- | ---- |
| `VehicleState.v == 0`（VCU が確認） | 走行中に ECU が止まらないように |
| Safety が EStop / Degraded でない | 異常時に更新しない |
| 他の ECU が更新中でない | 2 ECU 同時停止 = EStop を避ける |
| manifest の検証 OK（上の manifest 参照） | 互換性・ダウングレード |
| 更新中、VCU は DriverInput を 0 として扱う（保守状態） | 更新中に動かさない |

NG のときは `error_code` を返す（`NOT_STOPPED` / `BUSY` / `BAD_SIZE` / `HASH_MISMATCH` / `INCOMPATIBLE` / `DOWNGRADE` / `NO_SPACE` / `BOOT_FAILED` / `HEALTH_FAILED` / `TIMEOUT`）。

### ヘルスチェックと確定

- 試験適用の時間は `trial_ms`（既定 5000 ms。ADR-0003 の「試験適用」の例と揃える）
- 合格条件（案）:
  1. 新イメージが起動し、Alive を途切れなく送っている
  2. VCU の Watchdog が、その ECU を Degraded / EStop の原因にしていない
  3. 新イメージの自己診断が OK（段階1では、イメージ側の「ready」で代用）
- VCU が合格と判断したら Commit を送る。**Commit が `trial_ms` 内に来なければ ECU が自分でロールバックする**（VCU が止まっても、不確定なイメージが残らない）
- 不合格（Degraded / EStop、Alive 途絶、起動失敗）は即ロールバック

### バスと更新チャネル

| 種類 | 送信 → 受信 | 内容 | 種別 |
| ---- | ----------- | ---- | ---- |
| UpdateCtrl | VCU → 対象 ECU | Request / Activate / Commit / Abort | 制御バス（イベント駆動） |
| UpdateStatus | 対象 ECU → VCU | 状態、進捗（受信済みブロック数）、`error_code`、現在のバージョン | 制御バス（状態変化時 + 100 ms） |
| UpdateData | VCU → 対象 ECU | ブロック番号、ペイロード、CRC。ECU は ack を返す | **更新チャネル**（帯域・遅延・欠損・断を別設定） |

- 更新チャネルにも、既存の故障注入（遅延 / 欠損 / 断）を適用できるようにする
- 制御フレームのバスマトリクスへの追記は、実装PRの中で `07_ecu_architecture.md` に反映する

### ログ

- **既存の CSV は変えない**（golden 維持）。更新イベントは別ファイル（JSON Lines）に出す: `{t, ecu, event, from_version, to_version, detail}`
- `versions` 列（05 の「任意」項目）の追加と、Config version のログ化（05 の未実装項目）は、基準ログの更新手順（CONTRIBUTING.md）に従って別PRで行う

### シナリオ

- シナリオ JSON に `update` イベント（時刻、対象 ECU、パッケージ、注入する故障）を追加できるようにする

## 検証したいこと（完了条件の案）

**正常系**
- Steering の実装を v1.0.0 → v1.1.0 に更新すると、挙動（例: 舵角の制限）が変わり、イベントログに残る
- 更新を行わないシナリオでは、ログが基準ログと全行一致する

**異常系（すべて自動テスト）**
- 転送途中でバスが断 → 再開できる。Abort すると元のまま
- ハッシュ不一致 / 互換性 NG / ダウングレード → 拒否（active 不変）
- 起動できないイメージ → 試行回数切れで前のバージョンに戻る
- 起動はするが Alive を送らない / EStop を引き起こすイメージ → 不合格で戻る
- Commit が来ない（VCU 停止）→ ECU が戻る
- 走行中の Request → `NOT_STOPPED` で拒否
- 2 つ目の更新を同時に要求 → `BUSY` で拒否
- 更新中でも E-Stop・Safety 途絶時のフェイルセーフが従来どおり効く

## 実装の進め方（小さなPRで）

1. 本ADR（議論・決定。要確認事項を決める）
2. Rust: manifest の型・検証（サイズ、SHA-256、互換性、ダウングレード）と `boot_ctl`（A/B・trial・CRC）。単体テスト。挙動は不変（golden 一致）
3. Rust: ECU 側の更新ステートマシンと更新チャネル（ブロック転送・ack・再開）。ECU 単体のテスト
4. Rust: VCU の UpdateManager（前提条件、1 ECU ずつ、保守状態）と Watchdog 連携、故障注入（途中断、壊れたイメージ、起動失敗）
5. WASM API と GUI: パッケージ選択 → 配信の進捗 → 試験適用 → 確定 / ロールバック、壊れたイメージの注入、イベントログ表示
6. シナリオ JSON の `update` イベント、イベントログ（別ファイル）、必要なら CSV の `versions` 列
7. 段階2（別ADR）: 実 WASM モジュール化、署名、キャンペーン（複数 ECU の順次更新）

## Consequences

- 良い点: 「更新して → 試して → 壊れたら戻る」を、バス故障や ECU 停止と組み合わせて検証できる。ADR-0003 の「試験適用（自動ロールバック）」と同じ考え方を、コードの更新に広げられる。ECU 構成の載せ替え（ADR-0002 の案 B / C）と自然につながる
- 悪い点: 実装・テストが増える。VCU に UpdateManager、各 ECU に更新ステートマシンが入る。更新チャネルという仕様が増える。`sha2` などの依存が増え、WASM が大きくなる。C++ には反映しない（Rust のみ）
- 将来: 署名と鍵管理、差分更新、実 WASM モジュール化、複数 ECU のキャンペーン更新、VCU 自身の更新（Safety を含めない分割が前提）

## 要確認事項

1. **番号**: 本ADRを 0005 としてよいか（ADR-0004 は C++/Rust 境界で予約済み）
2. **イメージの実体**: 段階1は論理イメージ（案A）、段階2で実 WASM（案B）でよいか
3. **署名**: ADR-0003 は署名を非ゴールとした。本ADRは「ハッシュ検証は必須、署名は段階2以降」でよいか。破損検知（ハッシュ）と改ざん検知（署名）は別物として扱う
4. **Watchdog**: 緩めずに「`reboot_ms` < `heartbeat_timeout_ms`（1000 ms）」と規定してよいか。代替案は、Alive に maintenance フラグを持たせ、更新中の ECU を一時的に監視から外す方式（安全側の判断が増えるので非推奨）
5. **VCU の更新**: Safety を含む VCU は対象外でよいか（Safety を切り出せるまで）
6. **更新チャネル**: 制御バスと別にする案Yでよいか。帯域の既定値（例: 1 ブロック 256 B、1 ブロックあたりの遅延）
7. **`signal_if_version`**: どこで定義するか（`02_interfaces_signals.md` に版を追加し、manifest の `requires` と突き合わせる案）
8. **ヘルスチェック**: 合格条件と `trial_ms`（既定 5000 ms）、`trial_boots_left` の既定値
9. **ダウングレード**: デモ用に許可するか。ロールバック（自動で前の版に戻る）とは別の操作として扱う
