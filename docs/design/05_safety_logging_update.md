# 安全・ログ・更新（Safety / Logging / Update）

## Safety（v1）

- SafetySupervisorSWCがE-Stopを発行できることが必須
- 監視対象:
  - EngineSWC / BrakeSWC / SteeringSWC / VehicleDynamicsSWC / Com / TimeBase
- 監視手法:
  - 各部品がheartbeat_counterを増やす
  - Safetyが100msごとに前回値から進んでいるか確認
  - タイムアウト（`safety.heartbeat_timeout_ms`、既定 1000 ms）を超えたら「応答なし」。1つ → Degraded、2つ以上 → E-Stop（ラッチ）

実装状況: **実装済み**（C++ / Rust）。Rust では、バス上の Alive フレームを監視し、受信側の各 ECU にもフェイルセーフがある
（DriverInput 途絶 → 入力 0、Safety 途絶 → 自律的に E-Stop 相当）。詳細は [safety_swc.md](safety_swc.md)、[07_ecu_architecture.md](07_ecu_architecture.md)。

E-Stop時の挙動:
- drive_accel_cmd=0
- brake_decel_cmd=max_decel（またはVehicleDynamics内で強制減速）
- steer_angle_cmd=0（任意、もしくは保持）

## Logging（v1）

ログは最初から必須。

再現性: 同じ Config + 同じシナリオ（JSON）なら、毎回同じログになる。C++ の基準ログ 4 本（通常 + 故障注入 3 パターン）を
Rust がシナリオ JSON で全行再現することを、テストと CI（`golden-log`）で確認している。

- 目的:
  - 不具合解析（なぜそう動いたか）
  - 差し替え前後比較（SDV要件）
  - リプレイ（再現性）

CSV項目例:
- t
- throttle, brake, steer
- drive_accel_cmd, brake_decel_cmd, steer_angle_cmd
- x, y, yaw, v, yaw_rate, wheel_omega
- estop, system_state
- versions（任意）

## Update（v1：Config更新）

SDV v1として「更新で挙動が変わる」を成立させる。

- 対象: Config（最大舵角、最大減速度、最大加速度、抵抗係数など）
- 更新手順（最小）:
  1. 新Config受領（ローカルファイル上書きでも可）
  2. 構文検証・値域検証
  3. バックアップ作成
  4. 適用（次周期から有効）
  5. 失敗時ロールバック（バックアップ復帰）

- 実装（Rust、[ADR-0003](../adr/ADR-0003-config-nvm.md)）:
  - Config は JSON。未知の項目・範囲外は全体を拒否（項目パス付きの全エラー）
  - NVM は Active / Backup の 2 面 + 世代番号 + CRC32。更新が通ると `Backup ← Active`、`Active ← 新Config`
  - 適用は**再起動で有効**（実機のコンフィグ更新に合わせる）。起動時は Active → Backup → 既定値の順に読む
  - ロールバックは `Active ← Backup`（連続では戻れない）。GUI から更新・ロールバック・破損の注入を試せる

- 期待される成果:
  - 同一入力でもConfig変更により挙動が変わる（達成）
  - ログに「Config version」「適用時刻」が残る（**未実装**）
  - 更新直後に Degraded / EStop になったら自動でロールバック（試験適用、**未実装**）

## Update（v1+：SWC差し替え）

v1+で、Control系（例：速度制限/クルコン）を差し替え可能にする。

- SafetySupervisorは差し替え対象外（常駐）
- 差し替え対象候補:
  - Motion/Control相当のSWC（追加SWCとして導入）
- 方法:
  - 共有ライブラリ（プラグイン）または別プロセス（推奨は別プロセス）
