# ロードマップと完成条件（Roadmap & Definition of Done）

## 0. 骨格固定（最初のコミット）

成果物:
- Signal定義（02_interfaces_signals.mdと一致）
- RTE（Read/Writeの枠）
- TimeBase（10ms/20ms/100msタスク）
- Logging（CSV出力の枠）
- 空のSWC（Runnableの枠とバージョン文字列）

DoD:
- “何も動かなくても”ビルドでき、タスクが回る（ログに時刻が出る）

---

## 1. Engineのみ（最初の本体）

成果物:
- EngineSWCが throttle→drive_accel_cmd を出す
- VehicleDynamicsは簡易で良い（v += drive*dt、wheel_omega=v/r）

DoD:
- throttleを変えるとvとwheel_omegaが単調に変化
- ログに入力と状態が残る

---

## 2. Steeringのみ

成果物:
- SteeringSWCが steer→steer_angle_cmd を出す（飽和＋一次遅れ）
- VehicleDynamicsがyaw更新（vは一定でも可）

DoD:
- steer左右でyawの符号が変わる
- 舵角上限が効く

---

## 3. Brakeのみ

成果物:
- BrakeSWCが brake→brake_decel_cmd を出す
- VehicleDynamicsが減速（v>=0）

DoD:
- brakeで停止時間が変わる
- vが負にならない

---

## 4. 統合（縦→横→Pose）

成果物:
- Engine+Brakeでvを確定
- Steeringでyaw更新
- x,y更新
- wheel_omega整合

DoD:
- 直進・旋回・停止が一貫して成立
- 同一入力でリプレイ一致（決定性）

---

## 5. SDV v1（監視・更新・分割の入口）

成果物:
- SafetySupervisor（heartbeat監視→EStop）
- Config更新（ロールバック付き）
- UI/シナリオと疎結合（Comを整理、必要ならUDP）

DoD（SDV v1）:
- 監視停止でEStopが発動し、必ず停止できる
- Config更新で挙動が変わり、失敗時は戻る
- ログで更新前後比較が可能

---

## “完成”の定義（プロジェクトv1のゴール）

完成（SDV v1）とする条件は以下をすべて満たすこと。

1. DriverInput（throttle/brake/steer）で車両状態（v, x,y,yaw, wheel_omega）が変化する  
2. E-Stopが必ず効く（異常時、または手動）  
3. ログを用いて挙動が再現できる（同一入力→同一軌跡）  
4. Config更新で挙動が変わり、ロールバックが可能  
5. SWC分割と信号I/F固定（RTE越し）を満たしている
