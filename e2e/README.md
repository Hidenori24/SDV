# e2e

ブラウザで GUI（`web/`）を実際に動かす**少数のスモークテスト**です。ロジック（故障注入・Watchdog・Config・物理）は [`tests/integration/`](../tests/integration/README.md)（ビルド済みの WASM を Node で実行）が見るので、ここは**ブラウザでしか確かめられないもの**だけに絞っています（保守が最大のコストなので、増やさない）。

| テスト | 確かめること |
|---|---|
| シミュレータが起動する | WASM が読み込まれ、時間が進み、エラーが出ない |
| 設定 | 有効な設定の保存、範囲外の拒否（理由に項目名が出る）、ロールバック |
| 故障注入 | ECU が 2 つ止まると E-STOP になり、車が止まる（再生速度を上げて短くしている） |
| ステアの符号 | 左でヨーレートが正、右で負（GUI は右が + なので反転している） |
| 設定の試験適用 | 厳しすぎる Watchdog（200 ms）を保存 → 短いバス断 → 不合格になり、自動で既定値に戻る |
| FOTA デモ | 走行 → 停車中に更新 → 同じコースを再走行 |

全部で約 15 秒、CI の `e2e` ジョブは他のジョブと並列です（公開リポジトリなので Actions は無料）。

## 実行
```bash
cargo build --release --target wasm32-unknown-unknown --manifest-path rust/sdv-core/Cargo.toml
cp rust/sdv-core/target/wasm32-unknown-unknown/release/sdv_core.wasm web/   # web/ には置かれていない（git 管理外）
cd e2e && npm ci
npm test                                            # CI と同じ: Chrome（channel: chrome）を使う
E2E_BROWSER_PATH=/path/to/chromium npm test         # 手元の Chromium を指定する
```
- 依存は `playwright-core` だけ（ブラウザはダウンロードしない）。ランナーの Chrome か、`E2E_BROWSER_PATH` で指定した Chromium を使う
- `web/` を静的に配る小さなサーバーをテストが自分で立てる（`helpers.mjs`）。`web/` の外は配らない

## 書くとき
- テストごとに新しいブラウザコンテキスト（localStorage が別）で動かす。**コンソールエラーとページエラーが 0** であることを毎回確かめる（`favicon.ico` の 404 は除く）
- 実時間を待たない: 再生速度（`#spd`）を上げ、`waitForFunction` で状態を待つ。固定の `waitForTimeout` は故障の注入の長さなど、必要なときだけ
- **壊して確かめる**: テストを足したら、GUI を意図的に壊して失敗することを確認する（例: ステアの符号の反転でステアのテストが落ちる）

## 今のテストが見ていないこと
- 設定の保存がブラウザの `localStorage` に**永続化**されること（再読み込みで残るか）。保存の呼び出しを消しても今のテストは通る
- 見た目（レイアウト、ダークテーマ、スマホ幅）
