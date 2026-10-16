# rust/sdv-core

SDV の Model 層（engine / brake / steering / vehicle dynamics）と 10ms 固定ステップのシミュレーションループを Rust に移植したクレートです。
ネイティブでもブラウザ（WebAssembly）でも動きます。C++ 実装は参照実装として残しています。

```bash
cd rust/sdv-core
cargo test                                            # C++の参照ログとの照合を含む
cargo build --release --target wasm32-unknown-unknown # -> target/wasm32-unknown-unknown/release/sdv_core.wasm
```

## C++ との一致確認
`tests/golden/cpp_latest.csv` は `build/sdv_sim` が出力したログ（10秒・1000行）です。`tests/golden_cpp.rs` が同じデモシナリオを Rust で実行し、全行・全列の一致（浮動小数点の誤差内）を確認します。
C++ 側の挙動を変えた場合は、`sdv_sim` を再実行して golden を更新してください。

## WASM API（C ABI、wasm-bindgen 不要）
| 関数 | 説明 |
|---|---|
| `sdv_reset()` | 状態を初期化 |
| `sdv_use_scenario(on)` | 1: 内蔵デモシナリオ / 0: 外部入力 |
| `sdv_set_input(throttle, brake, steer)` | ドライバー入力 |
| `sdv_set_estop(on)` | E-Stop 要求 |
| `sdv_step(n)` | 10ms × n 進める |
| `sdv_out_ptr()` / `sdv_out_len()` | 出力バッファ（f32）: `t,x,y,yaw,v,yaw_rate,wheel_omega,throttle,brake,steer,drive_accel_cmd,brake_decel_cmd,steer_angle_cmd,estop,system_state` |

RustもTDDで進めます（先に失敗するテスト→最小実装→整理）。CIとPRのOK条件は [CONTRIBUTING.md](../CONTRIBUTING.md) を参照。
