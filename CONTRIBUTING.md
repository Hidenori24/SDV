# コントリビューションガイド

## 開発方針: TDD（Rust / C++ / C 共通）

すべての実装はテスト駆動で進めます。

1. **Red**: まず失敗するテストを書く（期待する振る舞いを先に固定）。
2. **Green**: テストが通る最小限の実装を入れる。
3. **Refactor**: テストが緑のまま、重複や命名を整える。

守ること:
- テストなしの実装変更はしない（バグ修正も、再現する失敗テストを先に書く）。
- コードを変えたら、関連ドキュメント（`docs/`、各 README）も同じPRで更新する。
- PRは小さく、動くものから順に出す。
- GUI（`web/`）を変えたら Chromium で実際に動かし、スクリーンショットをPRに添える。

## テストの実行方法

| 対象 | コマンド |
|---|---|
| C++（Catch2 / ctest） | `cmake -S . -B build && cmake --build build -j && ctest --test-dir build --output-on-failure` |
| Rust: sdv-core | `cd rust/sdv-core && cargo test`（C++ の基準ログとの全行照合を含む）、`cargo test --features fota-simulator` |
| Rust: sdv-fota | `cargo test --manifest-path rust/sdv-fota/Cargo.toml`（`--features host-signing`、`cargo check --no-default-features`、`rust/sdv-fota/tests/c_smoke.sh` も CI で実行） |
| Rust: sdv-picarx | `cargo test --manifest-path rust/sdv-picarx/Cargo.toml` |
| Rust: sdv-sec | `cd rust/sdv-sec && cargo test --features sign && ./tests/c_smoke.sh` |
| 結合テスト（ビルド済み WASM） | `cargo build --release --target wasm32-unknown-unknown --manifest-path rust/sdv-core/Cargo.toml && node tests/integration/run_wasm.mjs`（[tests/integration/README.md](tests/integration/README.md)） |

C のコードを追加する場合も同じ流れです。`CMakeLists.txt` に `C` 言語を有効化し、テスト実行ファイルを `add_test` で登録すれば CI の `ctest` に自動で含まれます。

### C++ の出力を変えたとき

シミュレータ出力が変わると `golden-log` ジョブが失敗します。意図した変更なら次で更新し、Rust側の実装も合わせます。

```
cmake -S . -B build && cmake --build build -j
mkdir -p logs && ./build/sdv_sim
cp logs/latest.csv rust/sdv-core/tests/golden/cpp_latest.csv
```

故障注入ログの golden（`cpp_stall_*.csv`）の再生成コマンドは `rust/sdv-core/tests/golden/README.md` を参照。

## CI とPRのOK条件

`.github/workflows/ci.yml`（PRと `main` への push で実行）:

- `cpp-tests`: ビルドと `ctest`
- `rust-tests`: `sdv-core` の `cargo test`（`fota-simulator` を含む）、`sdv-fota`、`sdv-picarx`、ホスト用署名ツール、no_std チェック、Classic 向け C ABI のスモークテスト、WASM ビルド
- `lint`: 全 crate の `cargo fmt --check` と `cargo clippy --all-targets`（**clippy のエラーは失敗、警告は当面許容**。`sdv-fota` / `sdv-picarx` には既存の警告が残っている。新しいコードで警告を増やさない。削減は別 PR で進め、いずれ `-D warnings` にする）
- `sec-tests`: `sdv-sec` の既知の値・フレーム MAC・ヘッダ一致、no_std の WASM ビルド、C ABI スモークテスト
- `integration`: ビルド済みの `sdv_core.wasm` を Node で動かす結合テスト（C++ の基準ログの全行再現 + シナリオ + 期待値）
- `aarch64`: Pi 5 の実ターゲット（aarch64-unknown-linux-gnu）でのビルドと、`qemu-user` でのテスト（手元で同じことをする手順は末尾）
- `golden-log`: C++ ログがコミット済みの基準ログと一致すること
- `ci-ok`: 上記すべての集約結果

新しい crate や CI ジョブを足したら、この一覧と上のテスト表、`README.md` も同じ PR で更新する。

**PRのマージ条件**: `ci-ok` が成功していること（ブランチ保護の必須ステータスチェックに `ci-ok` を指定する）。

## Pi 5（aarch64）向けのビルドとテスト

CI の `aarch64` ジョブと同じことを手元でやる手順（Linux / Ubuntu）。x86-64 では通るのに aarch64 でだけ失敗するコード（`O_NOFOLLOW` のようなアーキテクチャ依存の値）を見つけるため。

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user-binfmt
# テストが起動する子プロセスも動かすため、binfmt_misc に aarch64 を登録する
sudo mount -t binfmt_misc none /proc/sys/fs/binfmt_misc 2>/dev/null || true
sudo /usr/lib/systemd/systemd-binfmt /usr/lib/binfmt.d/qemu-aarch64.conf
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc QEMU_LD_PREFIX=/usr/aarch64-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu --manifest-path rust/sdv-picarx/Cargo.toml
cargo test  --target aarch64-unknown-linux-gnu --manifest-path rust/sdv-fota/Cargo.toml
```

エミュレーションでは時間依存のテストが不安定になる。CI の `aarch64` ジョブは次のようにしている（実機のテストの代わりにはならない）。
- `sdv-sec` / `sdv-core`: 全テストを実行
- `sdv-fota`: 全テストを実行。時間依存のテストが稀に揺らぐので最大 3 回試す（決定的な失敗は毎回落ちるので見逃さない）
- `sdv-picarx`: `--lib --bins` を実行。プロセスを起動して 10 ms の周期の期限を見る結合テストは、コンパイルだけ（x86-64 の `rust-tests` では実行される）
