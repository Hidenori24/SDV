# sdv-sec

SDV の通信の基礎ライブラリ（[ADR-0005](../../docs/adr/ADR-0005-fota.md)）。FOTA の署名検証と、バスのフレーム認証に使う。

| 機能 | 用途 |
| ---- | ---- |
| `sha256` | 破損検知、manifest の `sha256` |
| `ed25519_verify` | パッケージ・manifest の署名検証（ECU 側。持つのは公開鍵だけ）。検証してから解析する |
| `ed25519_sign` / `ed25519_public_key`（feature `sign`、ホスト専用） | 署名ツール、テスト、デモ鍵。**ECU 向けのビルドには含めない** |
| `hmac_sha256`、`ct_eq` | MAC、定数時間の比較 |
| `frame_mac` / `frame_mac_verify`、`Freshness` | SecOC 風のフレーム認証と、リプレイ防止（鮮度カウンタ） |

暗号の実装は自作せず、RustCrypto（`sha2` / `hmac` / `ed25519-dalek` / `subtle`）を使う。

## 作り（実機に載せる前提）
- `no_std`、ヒープなし、グローバルな状態なし。時刻・ファイルを使わない。入出力は呼び出し側のバッファ
- C ABI（`sdv_sec_*`）を [`include/sdv_sec.h`](include/sdv_sec.h) で公開する。Rust のシミュレータと、実機の C++ の ECU ソフトが**同じ実装**を呼べる（二重管理を避ける）
- 戻り値: `0` 成功 / 正の値は失敗の種類（`1` 鍵不正、`2` 署名不正、`3` タグ不正、`4` 古いカウンタ）/ `-1` ヌルポインタ・不正な引数

## フレーム MAC の仕様
```
tag = HMAC-SHA256(key, data_id (u16 BE) || freshness (u32 BE) || payload) の先頭 8 バイト
```
- `data_id` でフレームの種類に、`freshness` で 1 回の送信に結び付ける。タグは**定数時間**で比較する
- 受信側は、**MAC を検証してから**鮮度を確認する（認証できないフレームでカウンタを進めない）。`Freshness::accept` は「最後に受け入れた値より大きい」ときだけ受け入れる。`u32` が一巡する前に鍵を替える（鍵の更新は本ライブラリの範囲外）

## テスト
```bash
cargo test --features sign      # NIST / RFC 4231 / RFC 8032 の既知の値、改ざん・別の鍵・弱い鍵の拒否、フレーム MAC、ヘッダと export の一致
cargo build --release --target wasm32-unknown-unknown   # no_std（署名の機能なし）
./tests/c_smoke.sh              # 静的ライブラリを作り、C のプログラムからヘッダ経由で呼ぶ
```
- 既知の値は Python の `cryptography` でも突き合わせた
- `tests/header.rs`: ヘッダの関数・定数が `src/ffi.rs` の export と一致していること
- `tests/c_smoke.sh`: 言語をまたぐ結合テスト（C → ヘッダ → Rust）。CI の `sec-tests` で実行

## 範囲外
鍵の保管・ローテーション・失効、既存の制御フレーム（DriverInput など）への MAC の適用、Config 更新への署名。いずれも別の PR / ADR で扱う。デモ鍵は**公開されたテスト用**で、本番の鍵ではない。
