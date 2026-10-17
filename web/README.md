# web/

ブラウザで動く SDV シミュレータ。シミュレーション本体は Rust（`rust/sdv-core`）を WebAssembly にしたもので、サーバー不要です。

## ローカルで試す
```bash
cargo build --release --target wasm32-unknown-unknown --manifest-path rust/sdv-core/Cargo.toml
cp rust/sdv-core/target/wasm32-unknown-unknown/release/sdv_core.wasm web/
cd web && python3 -m http.server 8000   # http://localhost:8000
```

## 操作
- ステア符号: GUIは「右 = +」。モデル（C++ / Rust）の `steer` は「正 = 左旋回（反時計回り、yaw 増加）」で、GUIが渡すときに反転する。ステアリングホイールは実際の舵の向きに回る
- スライダー or キー: ↑/W アクセル、↓/S ブレーキ、←→/A,D ステア、Space E-Stop
- 「シナリオ再生」: シナリオ（既定は C++ 版と同じ内蔵デモ）を再生。リセットで最初から再生し直す
- 再生速度 0.25x〜4x、一時停止、リセット
- ECU構成（故障注入）: VCU / Engine ECU / Brake ECU / Steering ECU / Plant の構成図。ノードをクリックで停止/再開。「バス遅延」「バス欠損」「バス断」で車載バスの障害を注入（ECU→Plantのアクチュエータ線は直結で影響なし）。バスが途切れると各ECUは自律的にフェイルセーフ（入力0扱い / 自律EStop）に入り、戻れば自動復帰する。VCUのWatchdogは生存カウンタが1.0秒を超えて止まったノードを検出する
- Watchdog由来のE-STOP（1ノード停止 → DEGRADED、2つ以上 → E-STOP ラッチ、リセットで解除）でも、走行ビューに赤枠とバナーを表示
- シナリオ（入力・故障のタイムライン）: 右パネルの「シナリオ」を開き、JSON（`steps` / `after` / `events`）を編集して「検証して再生」。不正な内容は項目パス付きで拒否され、実行中のシナリオはそのまま。例ボタン（デモ / ECU2つ停止 / バス断 / E-Stop / 不正な例）。イベント（stall / resume / estop / estop_release / bus_down / bus_up / vcu_stall / vcu_resume）で起きた故障は、ECU構成図やバスのボタンにも反映される。同じシナリオなら毎回同じ走りになる（再現性）
- 設定（Config / NVM）: 右パネルの「設定」を開くと、「車体の諸元」「物理（世界の値）」「調整（ソフトの設定）」を別々の入力欄で編集できる（物理には路面の目安ボタン: フローリング・カーペット・濡れた路面・氷など）。その下の「導出値」（最小旋回半径・横加速度の上限・臨界速度と、速度ごとの旋回半径のグラフ）は編集中の値から自動で計算され、変更はできない。JSON を直接編集することもできる（旧形式も読める）。「検証して保存 → 再起動」で反映する。範囲外・未知の項目は理由つきで拒否され、何も保存されない。「ロールバック」でひとつ前に戻せる。保存先はブラウザの localStorage（疑似NVM: Active / Backup の2面 + CRC）。例ボタン（既定 / 高出力 / 厳しい監視 / 不正な例）、故障注入ボタン（Active破損 / Backup破損 / NVM消去）で「壊れたら Backup、両方だめなら既定値で起動」を確認できる。Config の値でグラフの縦軸・Watchdog 表示も変わる
- 走行軌跡（全体）: 軌跡全体を自動フィットして表示。「軌跡クリア」またはリセットで消去

## GitHub Pages
`.github/workflows/pages.yml` が `main` への push で WASM をビルドして公開します。
初回のみ、リポジトリの Settings → Pages → Build and deployment → Source を **GitHub Actions** にしてください。
