# タスク設計（Tasks & Scheduling）

## 目標

- “AUTOSARっぽく”周期Runnableを回す
- 実装はシンプルにし、後でOS/スレッド化できる形にする

## タスク周期（v1）

- 10ms: 制御・プラント更新（主周期）
- 20ms: 入力更新、UI同期（必要なら）
- 100ms: 安全監督、診断、状態遷移

## Runnable呼び出し順（v1推奨）

10msタスク:
1. EngineSWC_Main_10ms
2. BrakeSWC_Main_10ms
3. SteeringSWC_Main_10ms
4. VehicleDynamicsSWC_Step_10ms
5. Logging（10msごと or 20msごと）

20msタスク:
1. DriverInputSWC_Main_20ms
2. Logging（補助）

100msタスク:
1. SafetySupervisorSWC_Main_100ms
2. Diag/Watchdog更新
3. Nvm更新チェック（必要なら）

## 注意（E-Stop優先）

- estop=trueのとき、以下のいずれかを必ず保証する
  - (A) 各SWC出力を0に強制する（RTEまたはSWC側）
  - (B) VehicleDynamicsで最終的にv→0へ強制収束する
- 推奨は (A)+(B) の併用（上位で無効化し、プラント側でも安全側）

## リアルタイム要件（v1の考え方）

- 厳密なRT保証はv1では不要
- ただし「決定性（同一入力で同一出力）」を優先し、以下を避ける
  - 乱数の無管理利用
  - 非決定的なスレッド同期（v1は単一スレッド推奨）
