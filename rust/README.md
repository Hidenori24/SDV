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
- `bus.rs`: 仮想バス（CAN風。フレームは周期送信、受信側は最新値を保持、遅延ティック数を指定可）。バスマトリクスは ADR-0002
- `sim.rs`: ECU構成（ADR-0002 案A）。VCU（DriverInput + Safety）/ Engine ECU / Brake ECU / Steering ECU / Plant（VehicleDynamics）がそれぞれ自分の `Rte` を持ち、バスで信号をやり取りする。10ms固定ステップの順序: VCU送信 → 各ECU実行 → Plant実行 → VCUタスク（各段の間でバス配送）。遅延0・欠損0では1プロセス版と同じログになる（C++ golden と一致）
- `failsafe.rs`: ECUの受信側フェイルセーフ。DriverInput が200ms超途絶 → 入力を0扱い、Safety が100ms超途絶 → そのECUが自律的に EStop 相当の動作に入る（VCU停止・バス断でも止まる）。フレームが戻れば自動復帰
- バス障害: `Sim::set_bus_delay_ticks` / `set_bus_loss_permille`（決定的な疑似乱数で再現可能）/ `set_bus_down`、VCU停止: `set_vcu_stall`。**ECU→Plant のアクチュエータ線は直結**で障害の影響を受けない
- `tests/bus_faults.rs` / `tests/wasm_api.rs`: 遅延・欠損・バス断・VCU停止とフェイルセーフ、C ABI
- `tests/ecu_bus.rs` / `tests/bus.rs`: ECUごとのRTE、バスの配送・遅延・バスマトリクス
- `tests/swc_rte.rs`: SWC単体と、RTEが独立していること

## Config（車の諸元・物理・調整値・JSON）
[ADR-0003](../docs/adr/ADR-0003-config-nvm.md)（改訂1: schema 2）の Config（`config.rs`）。4 つに分ける。
- `vehicle_spec`（諸元）: ホイールベース、車輪半径、最大舵角、舵のサーボの遅れ、最高速度、最大加速度・減速度、E-Stop の減速。Plant と実機の ECU ソフトが使う
- `physics`（物理）: 重力、**タイヤの摩擦係数 μ**、走行抵抗。Plant だけが使う（実機では自然が決める）
- `tuning`（調整）: E-Stop 時の最大減速、Watchdog のタイムアウト、フェイルセーフのタイムアウト
- 導出値（`Config::derived()` / `derived_json()`、**読み取り専用**）: 最小旋回半径 `L/tan(δmax)`、横加速度の上限 `μ×g`、摩擦が効き始める速度、速度ごとの旋回の曲線

```json
{ "schema": 2,
  "vehicle_spec": { "wheelbase_m": 0.20, "wheel_radius_m": 0.03, "max_steer_angle_rad": 0.40, "steer_time_constant_s": 0.15,
                    "top_speed_mps": 3.0, "max_accel_mps2": 2.0, "max_decel_mps2": 4.0, "estop_decel_mps2": 6.0 },
  "physics": { "gravity_mps2": 9.81, "tire_friction_mu": 0.6, "drag_coeff": 0.15 },
  "tuning": { "estop_max_decel_mps2": 4.0, "safety": { "heartbeat_timeout_ms": 1000 },
              "failsafe": { "driver_timeout_ms": 200, "safety_timeout_ms": 100 } } }
```
- `config::parse(json)`: 欠けた項目は既定値、未知の項目は拒否、範囲外が1つでもあれば全体を拒否し、`項目パス: 理由` を**すべて**返す。`config::to_json` は常に schema 2 で書き出す
- **schema 1 は自動で schema 2 に変換して読む**（`"schema": 1`、または `schema` がなく旧形式のキーを含む場合）。範囲エラーは変換後の schema 2 のパスで報告する。旧形式と新形式のキーの混在は拒否
- **タイヤの限界**: Plant は有効な舵角を `atan(L×μ×g/v²)` に制限する（横加速度 ≤ μ×g）。既定値（μ=0.6）では 3 m/s のフルロックで 363°/s → 約 112°/s。C++ の Model と同じ式（暫定の写し）
- `Sim::with_config(cfg)` で起動、`Sim::set_config(cfg)` で入れ替え（再起動扱い）、`Sim::reset()` は同じ Config で再起動
- `tests/config.rs`: 形式・検証・境界値、schema 1 の変換、導出値、Config が挙動（加速・最高速度・旋回・摩擦・Watchdog・フェイルセーフ）を実際に変えること

## NVM（疑似不揮発メモリ）
`nvm.rs`: Config を A/B 2面（Active / Backup）+ 世代番号 + CRC32 で保存する（[ADR-0003](../docs/adr/ADR-0003-config-nvm.md)）。
- `commit(json)`: 検証に通ったものだけ保存（`Backup ← Active`、`Active ← 新Config`。保存するのは既定値を補った完全な形）。落ちたら何も変えず、全エラーを返す
- `load()`: Active が正常なら採用、壊れていれば Backup、両方だめなら既定値。どの面から読んだか・各面の状態（空 / 正常 / 破損）も返す
- `rollback()`: `Active ← Backup`、Backup は空にする（連続では戻れない）。世代番号は常に増える
- 記憶装置は `NvmStorage` で抽象化（テスト・WASM はメモリ、ブラウザは localStorage に保存する予定）
- `tests/nvm.rs`: CRC の標準値、コミット/ロールバック/フォールバック、破損・切り詰め・他形式のデータ、起動への利用
- 次のPR: WASM API と GUI

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

## FOTA package validation (ADR-0005)

`sdv-fota` is a separate crate so FOTA does not depend on the simulator. Its signed manifest
and v1 binary package format use `SDVF` magic, format version, key-ID length, manifest length,
payload length, 64-byte signature, key ID, exact signed manifest bytes, then payload.
`validate_package` verifies the raw manifest signature before parsing it, then checks target
ECU/SWC, semantic version and downgrade policy, security-version floor, signal interface and
Config schema compatibility, payload length, and SHA-256.

The FOTA code is split into domain types (`model.rs`), package validation (`package.rs`), update
orchestration (`service.rs`), and service-level platform ports (`ports.rs`): `ImageStorage`,
`BootControl`, `HealthMonitor`, and `PackageSource`.
`FotaInstaller` writes the validated image in bounded chunks to the inactive slot, streams it
back through a fixed-size buffer for SHA-256 verification, then schedules a trial boot. The
`stage_signed_manifest_from_source` path verifies the signed manifest before receiving payload
bytes, then streams payload chunks directly from the `PackageSource` port to inactive storage.
That path uses a fixed 1 KiB I/O buffer and rejects truncation, trailing bytes, and digest
mismatches before scheduling a trial boot.
`stage_package_from_source` additionally reads the SDVF header, signature, key ID, and manifest
from the same source. The caller supplies bounded manifest storage; envelope metadata and image
payload are not assembled into one package-sized allocation.
`resolve_trial` commits only after the health adapter reports success;
otherwise it requests rollback. `sdv-fota::linux` implements the Pi 5/Linux host backend for
application-image slots and redundant CRC-protected boot metadata in a configured Linux data directory. On the Pi 5 this
adapter does not write the OS, EEPROM, boot partition, or raw block devices. The
`sdv-fota-launcher` binary and example unit at `deploy/systemd/sdv-fota-launcher.service` select
and start an image, poll a configured health command during the manifest's trial window, commit a
healthy trial, and roll back an unhealthy one. Provisioning still needs to initialize boot
metadata, install the launcher and health check, and provide an executable signed application
image. The `sdv-fota` host command initializes boot metadata and streams a signed package file
into the inactive slot; it accepts the ECU compatibility values and public trust key as inputs.
The host-only `sdv-fota-sign` tool creates SDVF packages from image files using a protected
32-byte seed file and streams the payload with a 4 KiB buffer. Build it with
`cargo build --release --manifest-path rust/sdv-fota/Cargo.toml --features host-signing --bin sdv-fota-sign`;
the signing feature is not enabled in the target installer or no-std library builds. Keep the
signing seed in the release environment and provision only the matching public key to the ECU.
The common crate also compiles with
`--no-default-features` (`no_std` + `alloc`). This removes `std` dependencies, but it is not
heap-free and does not make the crate AUTOSAR Classic compatible. Flash I/O and read-back are
block-oriented; streaming avoids an image-sized allocation. Manifest text and supported schema
versions are deserialized into fixed-capacity inline fields (64 KiB maximum input, 16 schema
versions); parsing does not allocate per field. The broader host-facing domain still uses `alloc`
for boot metadata and Linux integration. Classic integration needs adapters for the selected
DCM, Csm/Crypto, NvM, MemIf/Fls, and bootloader.
See [the Classic architecture proposal](../docs/design/fota_classic_architecture.md). `SIGNAL_IF_VERSION` is currently 1.

## WASM API（C ABI、wasm-bindgen 不要）
| 関数 | 説明 |
|---|---|
| `sdv_reset()` | 状態を初期化 |
| `sdv_use_scenario(on)` | 1: 内蔵デモシナリオ / 0: 外部入力 |
| `sdv_set_input(throttle, brake, steer)` | ドライバー入力 |
| `sdv_set_estop(on)` | 外部 E-Stop 要求（ウォッチドッグ由来の EStop はラッチされ、`sdv_reset()` でのみ解除） |
| `sdv_set_bus(delay_ticks, loss_permille, down)` | 車載バスの遅延(10msティック)・欠損(‰)・断。ECU→Plant の直結線は対象外 |
| `sdv_set_vcu_stall(on)` | VCU（DriverInput + Safety）の停止/再開 |
| `sdv_boot()` | NVMから Config を読み込み（Active → Backup → 既定値）、その Config で再起動。戻り値: 0 Active / 1 Backup / 2 既定値 |
| `sdv_config_json()` / `sdv_default_config_json()` / `sdv_derived_json()` | 実行中 / 既定の Config の JSON、実行中の Config の導出値（最小旋回半径・横加速度の上限・旋回の曲線）をバッファに入れる（`sdv_buf_ptr` / `sdv_buf_len` で読む） |
| `sdv_derived_json_for(ptr, n)` | 保存前の編集中 Config（JSON）の導出値を計算してバッファに入れる。0 成功 / 1 不正（理由をバッファへ）/ 2 文字コード不正。何も保存・適用しない（GUI の導出値のライブ表示用） |
| `sdv_nvm_commit(ptr, len)` | JSON を検証して Active に保存。0 成功 / 1 不正（理由は1行1件でバッファ）/ 2 文字コード不正。適用は次の `sdv_boot` |
| `sdv_nvm_rollback()` | `Active ← Backup`。0 成功 / 1 バックアップなし |
| `sdv_nvm_slot_status(slot)` | -1 空 / -2 破損 / それ以外は世代番号（slot 0 Active, 1 Backup） |
| `sdv_nvm_read(slot)` / `sdv_nvm_write(slot, ptr, len)` / `sdv_nvm_erase(slot)` | 面の生バイトの取り出し・復元・消去（ブラウザの永続化用） |
| `sdv_nvm_corrupt(slot)` | 故障注入: 面の中央の1バイトを反転 |
| `sdv_alloc(n)` / `sdv_free(ptr, n)` | ホストが文字列・バイト列を渡すためのメモリ確保 |
| `sdv_set_scenario(ptr, len)` / `sdv_scenario_json()` / `sdv_demo_scenario_json()` | シナリオJSONの検証と設定（0 成功 / 1 不正（理由は1行1件でバッファ）/ 2 文字コード不正。不正なら実行中のシナリオは不変）、実行中・内蔵デモのJSON取得。`sdv_use_scenario(1)` で有効、`sdv_reset()` で最初から再生 |
| `sdv_set_stall(component, on)` | SWC の故障注入。component: 0 engine / 1 brake / 2 steering / 3 vehicledynamics |
| `sdv_step(n)` | 10ms × n 進める |
| `sdv_out_ptr()` / `sdv_out_len()` | 出力バッファ（f32）: `t,x,y,yaw,v,yaw_rate,wheel_omega,throttle,brake,steer,drive_accel_cmd,brake_decel_cmd,steer_angle_cmd,estop,system_state` に続けて、SWCごとの無応答時間 `silent_ms`（engine, brake, steering, vehicledynamics）。続いて ECU（engine, brake, steering）ごとのフェイルセーフ状態（bit0: DriverInput途絶, bit1: Safety途絶）。続いて注入中の故障（1.0 = 有効）: stalled[engine, brake, steering, plant, vcu], bus_down。全28要素 |

RustもTDDで進めます（先に失敗するテスト→最小実装→整理）。CIとPRのOK条件は [CONTRIBUTING.md](../CONTRIBUTING.md) を参照。
