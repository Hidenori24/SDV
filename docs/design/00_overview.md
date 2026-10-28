# 全体方針（Overview）

## ゴール

「SDVらしさ」を成立させる最小要件を満たしつつ、実装規模を制御して完成へ到達する。

SDVらしさ（v1）は次を指す。

- 機能が部品（SWC）として分割され、I/F（信号）が固定されている
- ログが残り、入力に対する挙動が再現できる
- 安全監督（SafetySupervisor）が異常時に必ず停止できる
- パラメータ更新（Config/NVM）で挙動が変化し、ロールバックできる

## 進捗管理

ロードマップに対応する進捗チェック。詳細な完成条件は [06_roadmap_dod.md](06_roadmap_dod.md) を参照。

- [x] 0. 骨格固定
  - RTE、TimeBase、Logging、各SWCの枠は実装済み
- [x] 1. Engineのみ
  - throttle → drive_accel_cmd → v / wheel_omega の更新が動作する
  - 単体テスト: test_engine_model.cpp / test_engine_swc.cpp
- [x] 2. Steeringのみ
  - steer → steer_angle_cmd（飽和＋一次遅れ）→ yaw の更新が動作する
  - 単体テスト: test_steering_model.cpp / test_steering_swc.cpp
- [x] 3. Brakeのみ
  - brake → brake_decel_cmd → v >= 0 を維持する
  - 単体テスト: test_brake_model.cpp / test_brake_swc.cpp
- [x] 4. 統合（縦→横→Pose）
  - Engine / Brake / Steering / VehicleDynamics の最小統合は実装済み（VehicleDynamics は縦・横とも Model 層に分離済み: `StepLongitudinal` / `StepLateral` / `StepVehicle`）
  - x, y, yaw, wheel_omega の更新が一貫して動作する
- [x] 5. SDV v1（Rust 実装で達成。C++ は参照実装として 1 プロセス構成のまま）
  - [x] SafetySupervisor: SWC別ハートビート監視（Watchdog）。1つ途絶 → Degraded、2つ以上 → EStop（ラッチ）
    - 単体テスト: test_safety_logic.cpp / test_safety_swc.cpp / test_watchdog_integration.cpp
    - 故障注入: `sdv_sim --stall <swc>:<t_s>`
  - [x] ECU 分割（VCU / Engine / Brake / Steering ECU + Plant、仮想バス、バス障害の注入、ECU の受信側フェイルセーフ）— [ADR-0002](../adr/ADR-0002-ecu-partitioning.md)、[07_ecu_architecture.md](07_ecu_architecture.md)
  - [x] Config/NVM 更新とロールバック（JSON・検証・A/B 2面 + CRC・破損時のフォールバック）— [ADR-0003](../adr/ADR-0003-config-nvm.md)
  - [x] UI / シナリオ入力の外部化（ブラウザ GUI、シナリオ JSON）
  - [x] 統合テスト・再現性確認（C++ の基準ログ 4 本を Rust が全行再現、同じシナリオなら同じログ、CI の `golden-log`）
- [ ] 6. 今後
  - [ ] 更新直後に Degraded / EStop になったら自動でロールバック（試験適用、ADR-0003 の後続）
  - [ ] ログへ Config の世代・適用時刻を記録
  - [ ] ECU 構成を設定で切り替える（案B・案C を試す）
  - [ ] FOTA: ECU ソフトの署名検証・更新・試験適用・自動ロールバック — [ADR-0005](../adr/ADR-0005-fota.md)（Accepted。`sdv-sec`、署名付きパッケージ検証、Pi 5 Linux向けA/B導入、HTTPS受信、trial launcher、root管理の信頼鍵ストア、非特権アプリ起動、順次キャンペーン方針を実装。PiCar-Xでは車両制御アプリ全体を`vehicle`対象として更新する。Robot HAT実機ドライバ・実Pi 5試験、実アプリhealth check、Classic BSW/MCAL/Bootloader結合は未完了。Classicを目標とした責務案は [fota_classic_architecture.md](fota_classic_architecture.md) を参照）

## 非ゴール（v1ではやらない）

- 実車相当の物理モデル（タイヤスリップ等の高精度）
- AUTOSAR規格完全準拠、ツール生成（ARXML等）
- 本格的なOTA配信基盤（クラウド/署名/差分配信）
- 自動運転の高機能（MPC、SLAM等）

## 用語

- SWC: Software Component。Runnable（周期関数）を持ち、RTE越しに信号を読んで書く
- RTE: SWC間の結合層。Read/Writeとスケジューラ呼び出しを提供
- BSW: 基盤サービス（時間、通信、NVM、監視、ログ）
- Plant: 車両ダイナミクス（VehicleDynamicsSWC）で表現される制御対象

## 開発原則

- “細かく”進めるため、常に「単体成立」→「統合」の順にする
- 信号I/Fは早期固定し、後から破壊的変更をしない
- 安全監督（E-Stop）を分離し、最後まで簡略化しない
- ログは最初から入れる（挙動が変わった理由を追える）
