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
| Rust | `cd rust/sdv-core && cargo test` |
| C++ ⇔ Rust ログ照合 | Rustの `cargo test` に含まれる（`tests/golden_cpp.rs`） |

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
- `rust-tests`: `cargo test` と WASM ビルド
- `golden-log`: C++ログがコミット済みの基準ログと一致すること
- `ci-ok`: 上記すべての集約結果

**PRのマージ条件**: `ci-ok` が成功していること（ブランチ保護の必須ステータスチェックに `ci-ok` を指定する）。
