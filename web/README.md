# web/

ブラウザで動く SDV シミュレータ。シミュレーション本体は Rust（`rust/sdv-core`）を WebAssembly にしたもので、サーバー不要です。

## ローカルで試す
```bash
cargo build --release --target wasm32-unknown-unknown --manifest-path rust/sdv-core/Cargo.toml
cp rust/sdv-core/target/wasm32-unknown-unknown/release/sdv_core.wasm web/
cd web && python3 -m http.server 8000   # http://localhost:8000
```

## 操作
- スライダー or キー: ↑/W アクセル、↓/S ブレーキ、←→/A,D ステア、Space E-Stop
- 「デモシナリオ」: C++ 版と同じ内蔵シナリオを再生
- 再生速度 0.25x〜4x、一時停止、リセット
- 走行軌跡（全体）: 軌跡全体を自動フィットして表示。「軌跡クリア」またはリセットで消去

## GitHub Pages
`.github/workflows/pages.yml` が `main` への push で WASM をビルドして公開します。
初回のみ、リポジトリの Settings → Pages → Build and deployment → Source を **GitHub Actions** にしてください。
