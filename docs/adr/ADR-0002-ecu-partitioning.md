# ADR-0002: 疑似ECU分割の方針

## Status
Accepted（案Aで進める。詳細は「決定事項」）

## Context
現状は全SWCが1つのプロセス・1つのグローバルRTEを共有している。実車では機能ごとに別ECUに載り、
ECU間は周期・遅延・欠損のあるバス通信になる。ECU間通信を入れると次ができるようになる。

- 通信途絶・遅延・古い値への対策（フェイルセーフ）を設計・検証できる
- ECU単位の故障注入（1ECUが落ちると、載っているSWCが同時に止まる）
- SDVの要点である「同じSWCを、ECU構成を変えて載せ替える」を試せる

ADR-0001 の「ECU分割に強い」という前提を、実際に試せる形にするのが目的。

### 前提・制約
- BSW全部をECUごとに複製はしない。**ECUごと**: RTE・スケジューラ・Diag/Watchdog。**共有（シミュレータ基盤）**: 時刻・ログ
- 主役は Rust（`rust/sdv-core`）。C++ は参照実装として「ECUなし」の現状を維持し、
  Rust の「ECUなし・バス遅延なし」設定でC++ログとの一致（golden）を保つ
- `VehicleDynamics` は制御対象（物理世界）であり、**ECUに載せない**（Plant）。ECUとはアクチュエータ指令 / センサ値でつなぐ
- 既存SWC（Engine / Brake / Steering / DriverInput / Safety）のコードはできるだけ変えない

## 検討した案

### 案A: 従来の機能ECU型（1機能 = 1ECU + ゲートウェイ）【推奨：最初に実装】
```
[VCU/GW ECU]       DriverInput, Safety
[Engine ECU]       Engine
[Brake ECU]        Brake
[Steering ECU]     Steering
         ↕ 仮想バス（Com）
[Plant]            VehicleDynamics
```
- 実車の ECM / ESC(ABS) / EPS / ゲートウェイに近く、分かりやすい
- 既存SWCがそのまま載る（新規SWC不要）。ECUとSWCが1:1なので、Watchdog の意味（1つ停止=Degraded、2つ以上=EStop）がそのまま保てる

### 案B: ドメイン統合型（3ECU）
```
[VCU ECU]          DriverInput, Safety
[Powertrain ECU]   Engine, Brake
[Chassis ECU]      Steering
```
- ECU数が少なく、バス信号も少ない
- Powertrain ECU が落ちると Engine と Brake が同時に止まる = 即EStop。「ECU単位の故障」の影響が大きく、検証しやすい
- ただし Brake は実車では Powertrain に属さないことが多く、現実との対応は弱い

### 案C: 中央集約 + ゾーン型（SDVらしい構成）
```
[Central Compute]  DriverInput, Safety, 制御ロジック（指令生成）
[Zone Front ECU]   アクチュエータドライバ（Steering, Brake）
[Zone Rear ECU]    アクチュエータドライバ（Engine）
```
- 近年のSDV/ゾーナルアーキテクチャに近い
- ただし現状のSWCは「制御ロジック」と「アクチュエータ指令」が一体のため、SWCの分割（新規SWC）が必要で変更量が大きい

### 比較
| 観点 | A 機能ECU | B ドメイン統合 | C 中央+ゾーン |
|---|---|---|---|
| ECU数 | 4 | 3 | 3 |
| 既存SWCの変更 | なし | なし | 分割が必要 |
| Watchdogの意味の維持 | ◎（そのまま） | ○（1ECU停止で2SWC） | △（再設計） |
| 実車との対応 | ◎ | ○ | ○（次世代） |
| 実装量 | 小〜中 | 小 | 大 |
| SDVらしさ（載せ替え） | ○ | ○ | ◎ |

## Decision（案）
1. **まず案Aを実装する。** 変更が最小で、Watchdog・golden 一致・既存テストの資産をそのまま活かせる。
2. **SWCのECU割り当てを「設定」にする**（SWC名 → ECU名の表）。実装後は B / C も設定の差し替えで試せるようにする。
   構成を比べられることこそ、SDV（ソフトでクルマを定義する）の見せ場になる。
3. 案Cは別ADRで改めて検討する（SWC分割が前提のため）。

## 仮想バス（Com）の仕様案
- フレーム単位の送受信。送信ECUの周期で送り、受信側は「最新値 + 受信からの経過時間」を持つ
- 設定可能: 遅延（ms）、欠損率、バス全体の停止
- 各フレームに生存カウンタ（alive counter）を持たせ、受信側が途絶を検出する（Watchdog はこれを監視する）

### バスマトリクス（案A）
| フレーム | 送信 | 受信 | 周期 | 内容 |
|---|---|---|---|---|
| DriverInput | VCU | Engine / Brake / Steering | 20 ms | throttle, brake, steer |
| Safety | VCU | Engine / Brake / Steering | 10 ms | estop, system_state |
| Alive_Engine / Alive_Brake / Alive_Steering | 各ECU | VCU | 10 ms | 生存カウンタ |
| VehicleState | Plant | VCU | 10 ms | t, x, y, yaw, v, yaw_rate, wheel_omega（センサ） |
| ActuatorCmd | Engine / Brake / Steering | Plant | 10 ms | 加速度・減速度・舵角（アクチュエータ指令。Plant との直結インターフェース） |

## フェイルセーフ（受信側の既定動作）
| 事象 | 動作 |
|---|---|
| DriverInput が途絶（> 200 ms 案） | 各ECUは入力を 0（スロットル0・ブレーキ0・舵角0）として扱う |
| Safety が途絶（> 100 ms 案） | 各ECUはローカルに EStop と同じ動作に入る（VCU停止時にも止まる） |
| 各ECUの Alive が途絶（> 1000 ms） | VCU の Watchdog が判定: 1ECU → Degraded、2ECU以上 → EStop（ラッチ）。現状と同じ |
| VehicleDynamics（Plant） | ECU ではないため Watchdog の監視対象から外す |

## 実装の進め方（小さなPRで）
1. 本ADR（議論・決定）
2. RTE をインスタンス化（ECUごとに別RTE）。SWC のコードは変えず、実行中のECUを切り替える方式。挙動は不変（ログは golden と一致）
3. 仮想バス（遅延0・欠損0）の導入。ここでも golden と一致
4. 遅延・欠損・ECU故障の注入 + フェイルセーフ（上表）
5. GUI: ECU構成図、各ECUの状態、バス遅延/欠損/ECU停止の操作

## Consequences
- 良い点: 通信途絶・ECU故障を検証できる。構成変更（載せ替え）を設定で試せる
- 悪い点: 実装・テストが増える。Watchdog の監視対象が SWC から ECU に変わる（Plant は対象外）。C++ には反映しない（Rust のみ）
- 要確認事項（下記）が決まるまでは実装しない

## 決定事項（2026-10-16）
1. 案A（VCU / Engine / Brake / Steering の4ECU）で始める
2. Brake は独立ECU
3. VCU 停止時は、各ECUが Safety 信号の途絶を検出して自律的に EStop 相当の動作に入る
4. バスは CAN 風（固定周期フレーム、最新値 + 経過時間 + 生存カウンタ）。Ethernet風（サービス指向）は将来の拡張
5. VehicleState（センサ）は VCU だけに配る（各ECUが必要になったら別途検討）
