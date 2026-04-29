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
  - 単体テスト: test_steering_swc.cpp
- [x] 3. Brakeのみ
  - brake → brake_decel_cmd → v >= 0 を維持する
  - 単体テスト: test_brake_model.cpp / test_brake_swc.cpp
- [x] 4. 統合（縦→横→Pose）
  - Engine / Brake / Steering / VehicleDynamics の最小統合は実装済み
  - x, y, yaw, wheel_omega の更新が一貫して動作する
- [~] 5. SDV v1（一部実装）
  - [x] SafetySupervisor: Watchdog タイムアウト → EStop を実装済み
  - [ ] Config/NVM 更新とロールバック
  - [ ] UI / シナリオ入力の外部化（現状は内蔵シナリオ）
  - [ ] 統合テスト・再現性確認

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
