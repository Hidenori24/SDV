# tools/video

リポジトリの説明動画を生成するスクリプト。

| 動画 | 内容 | ページ |
|---|---|---|
| [docs/media/sdv-intro.mp4](../../docs/media/sdv-intro.mp4)（82 秒） | 紹介: 構成、ECU 分割、安全、再現性、Config / FOTA、今後 | `intro.html` |
| [docs/media/sdv-sim-vs-real.mp4](../../docs/media/sdv-sim-vs-real.mp4)（85 秒） | シミュレータと実機: 共通部分と Platform の差し替え、両者の違い、実機の FOTA、実機で確認すること | `sim-real.html` |

1 枚の Web ページが「時刻 t の 1 フレーム」を描き（`window.renderAt(t)`）、ヘッドレス Chrome で 30 fps ぶん撮って ffmpeg でつなぐ。
背景は three.js の 3D、その上に 2D Canvas の HUD（文字・図・グラフ）を重ねている。

## 作り直す

```bash
tools/video/render.sh            # 2 本とも
tools/video/render.sh sim-real   # 片方だけ
```

必要なもの: Node 20+、Python 3（`numpy`・`scipy`）、ffmpeg、Chrome / Chromium。
Playwright 付属のブラウザを使わない場合は `CHROME_PATH=/usr/bin/google-chrome` のように指定する。

- GPU のない環境（SwiftShader）では 1 フレーム 1〜1.5 秒かかり、1 本（約 2500 フレーム）で 40〜60 分
- シーンごとに `out/<動画>_s1.mp4` … を作る。直したシーンのファイルだけ消して再実行すれば、そのシーンだけ撮り直す
- 3D の描画解像度は既定で 1440×810（HUD は 1920×1080）。`VIDEO_QUERY=gw=1920` で等倍、`gw=1280` で軽くなる

描画の確認だけなら、数フレームを静止画で出す（`python3 -m http.server 8123` を `tools/video/` で起動しておく）。

```bash
cd tools/video && node capture.mjs sim-real stills 5,14,27,40,50,64,76,83
```

## ファイル

| ファイル | 役割 |
|---|---|
| `intro.html` / `sim-real.html` | 動画本体。シーンの時間割は冒頭の `SC`、カメラの動きは `CK` |
| `hud.js` | HUD の共通部品（文字、パネル、引き出し線、カメラ用の補間） |
| `gen_golden.py` | 基準ログから `golden.js`（動画が読むデータ）を作る |
| `capture.mjs` | フレームを撮って、シーンごとの mp4 にする |
| `audio.py` | BGM を合成する（`audio_<動画>.wav`） |
| `render.sh` | 上記を順に実行し、1080p 版（`out/`）と 720p 版（`docs/media/`）を出す |

## 文言の方針

動画内の文言は、README・ADR・設計文書の書き方（平叙の説明）に合わせる。宣伝文句のような言い回しや、文書にない主張は入れない。
未確認のこと（実機での動作など）は、画面上でも「未確認」「想定」と書く。

## 動画の中身と、事実・演出の区別

### 紹介（intro.html）

| シーン | 内容 | 出どころ |
|---|---|---|
| 03 SAFETY | 車速のグラフ、Degraded / EStop になる時刻 | 基準ログ `cpp_stall_steering3_brake3.5.csv` をそのまま描画 |
| 04 REPLAY | 走行軌跡（x–y） | 基準ログ `cpp_latest.csv` をそのまま描画。C++ と Rust の線は同じデータ（全行一致を CI の `golden-log` と `cargo test` が確認しているため） |
| 04 のテスト数 | Rust 368 / C++ 90 | 2026-11 時点で `#[test]` と `TEST_CASE` を数えた値。**増減したら `intro.html` の数字を直す** |
| 06 ROADMAP | 実機、ECU 構成の切り替え、FOTA 段階2、Classic 結合 | [00_overview.md](../../docs/design/00_overview.md) の「今後」、ADR-0002 / ADR-0005、[hardware_candidates.md](../../docs/notes/hardware_candidates.md)。未完了の項目 |

- 3D の車は実車サイズの CG で、実機（PiCar-X）の見た目ではない。走行速度も演出用に拡大している

### シミュレータと実機（sim-real.html）

| シーン | 内容 | 出どころ |
|---|---|---|
| 02 / 03 の入力と車の動き | シミュレータ側の軌跡、実機側の車輪の回転と舵角 | 基準ログ `cpp_latest.csv` の同じ行を両方に与えている。**実機側は「同じ入力ならこう動くはず」という図で、実測ではない** |
| 03 TARGET | 入力の受け方、開ループ駆動、ブレーキ / E-Stop、異常時の停止要求 | [rust/sdv-picarx/README.md](../../rust/sdv-picarx/README.md)、[rust/README.md](../../rust/README.md)（`runtime.rs` / `runtime_host.rs`） |
| 04 DIFFERENCES | 違いの表 | 上記と [ADR-0003](../../docs/adr/ADR-0003-config-nvm.md)（physics は Plant だけが使う） |
| 05 FOTA | 受信 → 署名検証 → 書き込み → trial → health check → commit / rollback | [ADR-0005](../../docs/adr/ADR-0005-fota.md) の実装状況。Linux の統合テストで確認済み、実 Pi 5 では未確認 |
| 06 CHECKLIST | 実機で最初に確認すること | [hardware_candidates.md](../../docs/notes/hardware_candidates.md) の「安全」と「実機に向けてソフトでやること」 |

- 実機の 3D モデルは「小型の RC カー + 基板」を一般的な形で描いたもので、PiCar-X の外観を写したものではない
- 実機確認が済んだら、「未確認」「想定」の文言と、このファイルの記述を直して撮り直す
