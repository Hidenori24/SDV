# sdv-fota

SDV の FOTA（ECU ソフトの更新・試験適用・ロールバック。[ADR-0005](../../docs/adr/ADR-0005-fota.md)）の実装です。署名済みパッケージを**検証し、非稼働面に書き込み、試験起動して、health check の結果で確定または元に戻します**。

> **範囲外（誤解しないでください）**: 量産向けの FOTA ではなく、AUTOSAR Classic 準拠でもありません。実機（Pi 5 / PiCar-X）での動作確認は**未実施**です。Classic を目標にした責務の境界案は [fota_classic_architecture.md](../../docs/design/fota_classic_architecture.md)、実機の運用手順と現在の境界は [fota-pi5-operations.md](../../docs/design/fota-pi5-operations.md)（末尾の「Current boundary」）を見てください。未完了の一覧は [Issue #166](https://github.com/Hidenori24/SDV/issues/166)。

## 構成: `no_std` のコアと、Linux の層

| 層 | 内容 | ビルド |
|---|---|---|
| **コア**（`no_std`、allocator なし） | パッケージの枠と検証（`package.rs`）、固定容量の manifest パーサ（`manifest_parser.rs`）、更新状態機械（`service.rs`、`FotaInstaller`、`resolve_trial`）、複数 ECU の更新を一つずつ進める `CampaignManager`（`campaign.rs`）、外部とのポート（`ports.rs`）、C ABI（`ffi.rs`） | `cargo check --no-default-features` |
| **Linux の層**（`std`、`target_os = "linux"`） | A/B イメージと状態をファイルに持つ backend（`linux.rs`）、root 管理の信頼鍵ストア（`trust_store.rs`）、実行ファイル 3 つ | 既定（feature `std`） |

コアは、呼び出し側が渡す領域（固定長の manifest 領域など）だけを使います。イメージ全体をメモリに持ちません（payload を逐次書き込み、readback は増分 SHA-256 で照合します）。

外部とのやり取りは**ポート**（trait）に分かれていて、Linux の層はその一つの実装です。Classic 向けには、同じポートを BSW の API につなぎます（まだ採用製品がないので未実装）。

| ポート | 役割 |
|---|---|
| `ImageStorage` | イメージの消去・書き込み・読み戻し（固定長ブロック） |
| `BootControl` | 起動するスロットの選択、trial の予約、commit / rollback |
| `HealthMonitor` | 試験起動したイメージが健全かの判定 |
| `PackageSource` | パッケージの入力（ファイル、HTTPS のストリームなど） |
| `CampaignTransport` | 更新の司令（Request / Activate / Commit / Abort）を ECU へ届ける診断・通信の経路 |

## パッケージ（SDVF）

```
"SDVF" | format(u8) | key-ID 長(u8) | manifest 長(u32 BE) | payload 長(u64 BE) | Ed25519 署名(64 B) | key ID | manifest | payload
```

- 署名は **manifest のバイト列そのもの**に対するもの。**検証してから解析する**。末尾に余分なバイトがあれば拒否します
- manifest（JSON、最大 64 KiB）が、対象 ECU・SWC・版・`security_version`・payload のサイズと SHA-256 を持ち、署名で payload を拘束します
- 互換性（信号 I/F の版、Config schema）・ダウングレード・`security_version` の巻き戻しも検証します（[ADR-0005](../../docs/adr/ADR-0005-fota.md)）
- 暗号は [`sdv-sec`](../sdv-sec/README.md)（Ed25519 の検証と SHA-256）を使います

## 実行ファイル

| 実行ファイル | 役割 | feature |
|---|---|---|
| `sdv-fota` | 管理 CLI。`init`、`install`、`install-url`（HTTPS、mTLS 対応）、`status`、`verify-images`、`rollback-trial`、`record-active-integrity`、および ECU 候補用の `verify-candidate` / `promote-candidate` / `commit-candidate` / `revert-candidate` | `std` |
| `sdv-fota-launcher` | systemd から使うランチャー。起動するスロットを選び、非特権のユーザーでアプリを起動し、health check の結果で commit / rollback する | `std` |
| `sdv-fota-sign` | **ホスト用**の署名ツール。秘密鍵（seed）で署名済みパッケージを作る。**ECU 側には入れない** | `std` + `host-signing` |

使い方（データボリュームの用意、最初のイメージの導入、HTTPS / mTLS、ECU 別のイメージ、shadow 候補など）は [fota-pi5-operations.md](../../docs/design/fota-pi5-operations.md)、systemd のユニットは [`deploy/systemd/`](../../deploy/systemd/) にあります。

## C ABI

[`include/sdv_fota.h`](include/sdv_fota.h) に、Classic の C ベースの SW-C / BSW 統合から呼べる**同期 C ABI**があります。診断受信・NvM・MemIf/Fls・Bootloader・OEM の health 判定へのコールバックを渡します。

- `sdv_fota_stage_package`: 署名を検証し、payload を逐次書き込み、readback を照合して trial を予約する
- `sdv_fota_select_for_boot` / `sdv_fota_resolve_trial`: 起動するスロットの選択と、health の結果による commit / rollback
- `sdv_fota_campaign_*`: `CampaignManager`（開始・司令の発行・安全条件の再確認・checkpoint の保存と復元）

これは SDV の接続契約であって、AUTOSAR の標準 API ではありません。

## 複数 ECU の更新（`CampaignManager`）

車両停止・Safety が正常・他に更新中の ECU がない、を開始条件に、最大 `MAX_CAMPAIGN_ECUS`（4）個の更新対象を**一つずつ** Request → Activate → Commit と進めます。`recheck` を周期タスクから呼ぶと、車両が動いた・Safety の状態が変わったときに対象へ Abort を出します。17 バイトの checkpoint（CRC32 つき）を NvM に保存・復元でき、VCU の再起動後も続けられます。`sdv-core` の optional feature `fota-simulator` が、これを模擬の車速と VCU の Safety 状態につなぎます。VCU 本体への統合と、通信の経路（`CampaignTransport` の実装）は未完了です（[#153](https://github.com/Hidenori24/SDV/issues/153)、[#154](https://github.com/Hidenori24/SDV/issues/154)）。

## テスト

```bash
cd rust/sdv-fota
cargo test                                   # 既定（std）
cargo test --features host-signing           # 署名ツールを含む
cargo check --no-default-features            # コア（no_std、allocator なし）
./tests/c_smoke.sh                           # 静的ライブラリを作り、C のプログラムからヘッダ経由で呼ぶ
```

- `tests/update_flow.rs`: 署名 → 検証 → 書き込み → trial → commit / rollback の流れ（メモリ上の fake）
- `tests/linux_backend.rs`: Linux の backend とランチャー（ファイル、権限、シンボリックリンク、ロック、子プロセスの起動、health check）
