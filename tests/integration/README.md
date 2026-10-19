# 結合テスト（シナリオ + 期待値）

「この操作・この故障を起こすと、こうなる」を **JSON のデータ**で書き、シミュレータ全体（ECU 分割・仮想バス・Watchdog・フェイルセーフ・Config・物理）を通して確かめる。ブラウザは使わない。

| ランナー | 何を動かすか | 実行 |
| -------- | ------------ | ---- |
| `run_wasm.mjs`（Node） | **ビルド済みの `sdv_core.wasm`**（ブラウザに配る成果物そのもの） | `node tests/integration/run_wasm.mjs`（先に WASM をビルド） |
| `rust/sdv-core/tests/integration_scenarios.rs` | 同じ C ABI をネイティブで | `cargo test`（`rust/sdv-core`） |

同じケースのファイルを両方が評価するので、ネイティブと WASM の差は「片方だけ失敗」として現れる。CI の `integration` ジョブが Node 側を、`rust-tests` が Rust 側を動かす。

`run_wasm.mjs` はさらに、**WASM が C++ の基準ログ 4 本（通常 + 故障注入 3 パターン）を全行再現する**ことも確かめる（`rust/sdv-core/tests/golden/`）。

## ケースの書き方（`scenarios/*.json`）
```json
{
  "name": "fault_two_ecus_stall",
  "description": "何を確かめるか（日本語で）",
  "seconds": 12,
  "config": { "physics": { "tire_friction_mu": 0.2 } },
  "scenario": { "steps": [ ... ], "events": [ ... ] },
  "expect": [ ... ]
}
```
- `scenario`: ドライバー入力の時系列と、時刻指定のイベント（stall / bus_down / estop / vcu_stall など）。形式は `rust/README.md` のシナリオと同じ。`steer` は**モデルの符号（正 = 左旋回）**
- `config`（省略可）: NVM に保存してから起動する部分 Config。既定は既定値
- `seconds`: 何秒ぶん動かすか（10 ms ごとに 1 行）

### 期待値（`expect`）
| `kind` | 意味 |
| ------ | ---- |
| `at` | 時刻 `t` の値が条件を満たす |
| `always` | `from`〜`to` の全ての時刻で条件を満たす |
| `first` | 条件が**最初に**真になる時刻が `between: [a, b]` の中にある（早すぎても遅すぎても失敗。検出の遅れの確認に使う） |

条件は `signal` / `op` / `value`。`op` は `==` `!=` `<` `<=` `>` `>=` `approx`（`approx` は `tol` が必要）。信号名は `lib.mjs` の `SIGNALS`（出力バッファの並び）と、`lateral_accel`（= `v × yaw_rate`）。

- **`first` は、初期値で既に真になる条件には使わない**（最初の行で真になってしまう。`v == 0` など。`at` / `always` を使う）
- 時刻は 10 ms 刻みの浮動小数点なので、半ティックの許容つきで比べる
- 期待値は「実装が今こうなるから」ではなく、**設計上こうなるべき**もので書く（例: 横加速度は `μ·g` を超えない、検出はタイムアウトの後）。窓を狭くしすぎると、無関係な変更で壊れる

## ケースを足すとき
1. `scenarios/` に JSON を足す。ランナーは追加不要
2. `node tests/integration/run_wasm.mjs` と `cargo test --test integration_scenarios` の両方で通ることを確認
3. **期待値をわざと壊して、失敗することも確認する**（テストが機能している証拠）

## 今あるケース
デモ走行 / ECU 2 つ停止（DEGRADED → E-STOP）/ バス断からの復帰 / VCU 停止 / E-Stop と解除 / フルロックでの摩擦の上限 / 氷の路面（Config → 挙動）/ 左右の符号規約
