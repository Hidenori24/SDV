# rust/sdv-core

SDV の Model 層（engine / brake / steering / vehicle dynamics）と 10ms 固定ステップのシミュレーションループを Rust に移植したクレートです。
ネイティブでもブラウザ（WebAssembly）でも動きます。C++ 実装は参照実装として残しています。

```bash
cd rust/sdv-core
cargo test                                            # C++の参照ログとの照合を含む
cargo build --release --target wasm32-unknown-unknown # -> target/wasm32-unknown-unknown/release/sdv_core.wasm
```

## 構成（RTE / SWC / Sim）
- `rte.rs`: 信号の入れ物 `Rte`（DriverInput / ActuatorCmd / VehicleState / Safety）。**ECUごとに1インスタンス**持つ想定（[ADR-0002](../docs/adr/ADR-0002-ecu-partitioning.md)）
- `swc.rs`: Engine / Brake / Steering / VehicleDynamics / DriverInput / Safety の各SWC。渡された `Rte` だけを読み書きする
- `sim.rs`: 10ms固定ステップのスケジューラ。現状は1つの `Rte` を全SWCで共有（ECU分割は次のPRで導入）
- `tests/swc_rte.rs`: SWC単体と、RTEが独立していること

## Safety（ウォッチドッグ）
C++ の `src/model/safety_logic.h` と `Swc::Safety`（`safety_swc.cpp`）を移植済みです。
- `model.rs`: `update_heartbeat` / `count_failed` / `compute_safety_state`（生存カウンタの停止検出と Normal/Degraded/EStop 判定）
- `safety.rs`: `SafetySupervisor::main_100ms`（外部E-Stop要求とウォッチドッグのラッチを統合。ラッチは `reset()` でのみ解除）
- `tests/safety_cpp.rs`: C++ の `test_safety_logic.cpp` と `test_safety_swc.cpp`（Watchdog部）を同じ期待値で移植
- `sim.rs`: `Sim` に組み込み済み。各SWCは実行時に生存カウンタを加算し、100msごとに Safety が監視する
  - 故障注入: `Sim::set_stall(Component, bool)`（C++ の `sdv_sim --stall` 相当。止めたSWCは出力を更新せず、生存報告もしない。VehicleDynamics を止めると時刻も止まる）
  - 外部E-Stop要求: `Sim::set_estop_request`、リセット: `Sim::reset`（ラッチと注入した故障も解除）
  - 状態確認: `Sim::swc_status()`（SWCごとの無応答時間と生死）
  - `tests/sim_watchdog.rs`: C++ の故障注入ログ（`tests/golden/cpp_stall_*.csv`）との全行照合ほか

## C++ との一致確認
`tests/golden/cpp_latest.csv` は `build/sdv_sim` が出力したログ（10秒・1000行）です。`tests/golden_cpp.rs` が同じデモシナリオを Rust で実行し、全行・全列の一致（浮動小数点の誤差内）を確認します。
C++ 側の挙動を変えた場合は、`sdv_sim` を再実行して golden を更新してください。

故障注入の golden（`tests/golden/cpp_stall_*.csv`）も同様です。再生成コマンドは `tests/golden/README.md` を参照してください。

## WASM API（C ABI、wasm-bindgen 不要）
| 関数 | 説明 |
|---|---|
| `sdv_reset()` | 状態を初期化 |
| `sdv_use_scenario(on)` | 1: 内蔵デモシナリオ / 0: 外部入力 |
| `sdv_set_input(throttle, brake, steer)` | ドライバー入力 |
| `sdv_set_estop(on)` | 外部 E-Stop 要求（ウォッチドッグ由来の EStop はラッチされ、`sdv_reset()` でのみ解除） |
| `sdv_set_stall(component, on)` | SWC の故障注入。component: 0 engine / 1 brake / 2 steering / 3 vehicledynamics |
| `sdv_step(n)` | 10ms × n 進める |
| `sdv_out_ptr()` / `sdv_out_len()` | 出力バッファ（f32）: `t,x,y,yaw,v,yaw_rate,wheel_omega,throttle,brake,steer,drive_accel_cmd,brake_decel_cmd,steer_angle_cmd,estop,system_state` に続けて、SWCごとの無応答時間 `silent_ms`（engine, brake, steering, vehicledynamics）。全19要素 |

RustもTDDで進めます（先に失敗するテスト→最小実装→整理）。CIとPRのOK条件は [CONTRIBUTING.md](../CONTRIBUTING.md) を参照。
