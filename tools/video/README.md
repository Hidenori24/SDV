# tools/video

リポジトリ紹介動画（[docs/media/sdv-intro.mp4](../../docs/media/sdv-intro.mp4)、82 秒）を生成するスクリプト。

1 枚の Web ページ（`index.html`）が「時刻 t の 1 フレーム」を描き、ヘッドレス Chrome で 30 fps ぶん撮って ffmpeg でつなぐ。
背景は three.js の 3D（夜の道路と車）、その上に 2D Canvas の HUD（文字・図・グラフ）を重ねている。

## 作り直す

```bash
tools/video/render.sh
```

必要なもの: Node 20+、Python 3（`numpy`・`scipy`）、ffmpeg、Chrome / Chromium。
Playwright 付属のブラウザを使わない場合は `CHROME_PATH=/usr/bin/google-chrome` のように指定する。

GPU のない環境（SwiftShader）では 1 フレーム 1 秒前後かかり、全 2460 フレームで 20〜40 分。
シーンごとに `out/s1.mp4`〜`out/s8.mp4` を作るので、直したシーンのファイルだけ消して再実行すれば、そのシーンだけ撮り直す。

描画の確認だけなら、数フレームを静止画で出す（`render.sh` と同じく `python3 -m http.server 8123` を `tools/video/` で起動しておく）。

```bash
cd tools/video && node capture.mjs stills 4.5,24,33,46,57,70,79
```

## ファイル

| ファイル | 役割 |
|---|---|
| `index.html` | 動画本体。`window.renderAt(t)` が時刻 t のフレームを描く。シーンの時間割は冒頭の `SC` |
| `gen_golden.py` | 基準ログから `golden.js`（動画が読むデータ）を作る |
| `capture.mjs` | フレームを撮って、シーンごとの mp4 にする |
| `audio.py` | BGM を合成する（`audio.wav`） |
| `render.sh` | 上記を順に実行し、1080p 版（`out/`）と 720p 版（`docs/media/`）を出す |

## 動画の中身と、事実・演出の区別

| シーン | 内容 | 出どころ |
|---|---|---|
| 03 SAFETY | 車速のグラフ、Degraded / E-Stop になる時刻 | 基準ログ `cpp_stall_steering3_brake3.5.csv` をそのまま描画 |
| 04 REPRODUCIBILITY | 走行軌跡（x–y） | 基準ログ `cpp_latest.csv` をそのまま描画。C++ と Rust の線は同じデータ（全行一致を CI の `golden-log` と `cargo test` が確認しているため） |
| 04 のテスト数 | Rust 368 / C++ 90 | 2026-11 時点で `#[test]` と `TEST_CASE` を数えた値。**増減したら `index.html` の数字を直す** |
| 06 ROADMAP | 実機、ECU 構成の切り替え、FOTA 段階2、Classic 結合 | [00_overview.md](../../docs/design/00_overview.md) の「今後」、ADR-0002 / ADR-0005、[hardware_candidates.md](../../docs/notes/hardware_candidates.md)。構想・未完了の項目 |

- 3D の車は実車サイズの CG で、実機（PiCar-X）の見た目ではない。走行速度も演出用に拡大している
- 実装状況が変わったら（実機確認が済んだ、など）、該当シーンの文言を直して撮り直す
