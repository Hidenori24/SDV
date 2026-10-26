# AUTOSAR Classic を基準にした ECU / FOTA アーキテクチャ

## 目的と適用範囲

FOTA の責務と ECU 内のソフトウェア境界を AUTOSAR Classic の構造に近づけるための設計案。現行実装が AUTOSAR Classic 準拠であるとは主張しない。Classic BSW 製品、生成 RTE、MCAL、Bootloader と結合する前の、移植可能な機能モデルを定義する。

## ECU 内の層

```text
Application Layer
  SW-C / Runnable / Port
       │ RTE API（生成・設定された接続）
       ▼
RTE
       │
       ▼
Basic Software
  Services: Com, Dcm, Csm/Crypto, NvM, Dem, WdgM, EcuM/BswM, OS
  ECU Abstraction: ECU 固有デバイスをMCAL APIへ隠蔽
  MCAL: Fls等のマイコン周辺ドライバ
       │
       ▼
MCU / Flash hardware

独立 Bootloader
  起動バンク選択、試行起動、確定または復帰
```

Classic の FOTA 全体が単一の標準 AUTOSAR モジュールとして定義されている、という扱いにはしない。更新キャンペーン、パッケージ、Bootloader との契約は OEM / Tier-1 / BSW vendor の責任境界として明示する。

## FOTA の配置と責務

- VCU上の更新オーケストレーションは、車両状態・更新順序を扱う上位制御として配置する。制御SW-CやSafetySupervisorから分離する。
- 更新対象ECUには、OEM定義の ECU Reprogramming / FOTA BSWサービスを置く。DCM/診断セッション経由で要求を受け、署名検証、適合性確認、書込み、起動要求、結果通知を管理する。
- DCMは診断通信・プログラミング要求の入口、Csm/Cryptoは暗号サービス、NvMは更新メタデータ、MemIf/Flsは不揮発メモリへのアクセスを担う。実際のAPIと書込みモデルは採用BSW/MCAL製品に合わせる。
- Bootloaderはアプリケーションの更新サービスから独立して、起動時のイメージ選択、試行回数、確定、復帰を担う。アプリケーションが更新状態を書き換えても、Activeイメージを破壊しない契約にする。
- 安全監視、車両停止条件、更新の許可判定は更新制御と分離し、既存Safetyのフェイルセーフを弱めない。

## 現行コードとの境界

| 現行実装 | 現在の役割 | Classicへ近づける際の位置づけ |
|---|---|---|
| `rust/sdv-core` の SWC / RTE | 独自の機能モデル・ECUシミュレーション | SW-C仕様・ポート契約の検討材料。AUTOSAR生成RTEではない |
| `rust/sdv-fota` の検証・更新処理 | 独自FOTAサービス | ECU更新BSWのポリシー/状態遷移の試作。標準BSW APIではない |
| `ImageStorage` / `BootControl` | 独自Rust trait。Flash read/writeは固定長block API | 現状はサービス境界。Classic実装ではNvM、MemIf/Fls、Bootloader APIへ結合する |
| `rust/sdv-fota/src/linux.rs` | Linuxファイル領域にA/Bイメージと状態を保存 | Pi 5開発・シミュレーション用バックエンド。MCALやBootloaderの代替ではない |
| `sdv-sec` | no_std・ヒープなしの暗号基礎 | Csm/Cryptoとの接続方式を選ぶ。BSW Csm APIを直接実装したものではない |

## Classic用ポート契約

FOTA状態機械はサービス要求を発行し、Classic統合側が選定BSW/MCALのAPIへ接続する。独自の `ImageStorage` 一つに全てを集約しない。

| FOTA側の責務 | Classic側の接続先 | 契約の要点 |
|---|---|---|
| 診断要求・進捗 | DCM、Com、OEM診断仕様 | セッション、認証、順序、タイムアウト、応答コード |
| 署名・ハッシュ | Csm/Crypto または認定済みcrypto実装 | 信頼鍵、アルゴリズム、同期/非同期処理、鍵ローテーション |
| 更新状態・単調増加値 | NvM | 冗長性、CRC、世代番号、書込み完了・電源断時挙動 |
| イメージ消去・書込み・読戻し | MemIf/Fls、ECU Abstraction、MCAL | アラインメント、ブロックサイズ、消去範囲、ECC、失敗通知 |
| 起動バンク・trial/commit/rollback | Bootloader/OEM Boot Manager | 起動メタデータ形式、試行回数、復帰条件、確定権限 |
| 更新許可・安全状態 | VCU/SafetySupervisorとの契約 | 車両停止、EStop/Degraded、同時更新禁止 |

Flash write/readbackは固定長block単位になり、readbackは増分SHA-256で処理する。SDVF受信は呼び出し側のmanifest領域を使い、イメージ全体をメモリに保持しない。マニフェストは固定容量JSON parserで解析し、入力は64 KiB、schema一覧は16要素までに制限する。共有FOTA状態、manifest、ポート、検証処理は `no_std` かつallocatorなしでビルドできる。Linux adapterとホスト向けpackage encoderは別featureで `std`/`alloc` を使う。Pi 5ホスト向けにはsystemdからスロット選択、起動、health check、trial commit/rollbackを実行するlauncherを用意する。`no_std` でビルドできることだけでは Classic 向け成立を意味しない。

## Raspberry Pi 5 の位置づけ

Pi 5上のLinux実装は、FOTA状態機械とインターフェースを開発・検証するホスト環境として維持できる。Pi 5 + Linux自体をAUTOSAR Classic ECUとは呼ばない。実Classicターゲットには、対象ハードウェアをサポートするAUTOSAR Classic OS/BSW/MCAL製品とBootloaderが必要になる。Pi 5をそのまま量産Classic ECUにする場合は、供給元が対応するClassic stack/OS/driver構成の有無を別途確認する。

この方針では、Linux adapterは `HostPlatform` 実装として残し、将来の Classic BSW adapter と対等な開発バックエンドにする。製品統合時にLinux用のファイル更新をFls/NvM/Bootloaderへ直接流用しない。

## 車両載せ替え時の変更範囲

| 変更 | 維持しやすい資産 | 車両/ECU側で変わる資産 |
|---|---|---|
| 同じECUとClassic stackで車両だけ変更 | SW-C内部ロジック、安定したポート仕様、共通FOTA状態機械 | ECU Extract、RTE接続/生成設定、Com/診断設定、信号割当、校正、互換性データ |
| 同じSW-Cを別ECUハードウェアへ移植 | SW-Cと標準RTE/BSW APIに依存する部分 | MCAL、ECU Abstraction、Flash/NvM設定、Bootloader、OS/タスク設定、メモリ配置 |
| 別BSW vendor / Classic releaseへ変更 | 要求仕様、独立した状態遷移とデータ形式 | BSW API binding、生成設定、コンフィグ、暗号・診断・メモリ統合と適合試験 |

再利用の条件は「上位が抽象的だから」だけではない。SW-Cポート、診断、メモリ、起動、Variant設定の契約とバージョンが管理され、統合時に検証されていることが必要。

## 実装を近づける段階

1. **Classic境界を固定**: SW-C/RTE/BSW/ECU Abstraction/MCAL/Bootloader間の責務と型・エラー契約を文書化する。
2. **FOTA内部を分離**: 状態機械・互換性ポリシーと、Crypto/NvM/Flash/Bootloader接続を別モジュールにする。Linux backendはホスト試験用として維持する。
3. **固定メモリ化**: 共有FOTA状態機械はallocatorなしで動き、SDVF parserはcaller bufferとinline値のみを使う。Classic adapter側も生成設定とBSW契約の範囲で静的メモリを割り当てる。
4. **Classic BSW adapter**: 採用製品のC API/生成コードとの結合層を作る。製品のAPI/MCALが決まる前に仮のAPIをAUTOSAR標準として固定しない。
5. **Bootloader契約と実機検証**: 書込み範囲、A/B bank、trial boot、電源断復旧、Commit/rollbackをターゲットで検証する。

## 適合性に関する注意

本設計資料はClassicアーキテクチャを目標にした境界案であり、AUTOSAR適合・機能安全認証・サイバーセキュリティ適合を表すものではない。適合性は採用リリース、BSW/MCAL/OS vendor、生成設定、OEM要求、検証成果物を含めて判断する。
