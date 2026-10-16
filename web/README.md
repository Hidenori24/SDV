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
- 「デモシナリオ」: C++ 版と同じ内蔵シナリオを再生
- 再生速度 0.25x〜4x、一時停止、リセット
- ECU構成（故障注入）: VCU / Engine ECU / Brake ECU / Steering ECU / Plant の構成図。ノードをクリックで停止/再開。「バス遅延」「バス欠損」「バス断」で車載バスの障害を注入（ECU→Plantのアクチュエータ線は直結で影響なし）。バスが途切れると各ECUは自律的にフェイルセーフ（入力0扱い / 自律EStop）に入り、戻れば自動復帰する。VCUのWatchdogは生存カウンタが1.0秒を超えて止まったノードを検出する
- Watchdog由来のE-STOP（1ノード停止 → DEGRADED、2つ以上 → E-STOP ラッチ、リセットで解除）でも、走行ビューに赤枠とバナーを表示
- 走行軌跡（全体）: 軌跡全体を自動フィットして表示。「軌跡クリア」またはリセットで消去

## GitHub Pages
`.github/workflows/pages.yml` が `main` への push で WASM をビルドして公開します。
初回のみ、リポジトリの Settings → Pages → Build and deployment → Source を **GitHub Actions** にしてください。
