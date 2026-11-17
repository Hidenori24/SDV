# Raspberry Pi の初回セットアップと、車体なしでできる確認

Raspberry Pi 本体と SD カードがある状態から、**車体（PiCar-X / Robot HAT）をつながずに**確かめられることを先に済ませる手順です（[#186](https://github.com/Hidenori24/SDV/issues/186)。親は [#160](https://github.com/Hidenori24/SDV/issues/160)）。

> **この手順は実機ではまだ誰も通していません。** スクリプト（`tools/pi/bringup_check.sh`）は x86-64 の Linux で通ることを確認していますが、Pi の上での動作・所要時間は未確認です。OS の書き込みや `raspi-config` の部分は、Raspberry Pi の一般的な手順をもとに書いています。つまずいた箇所があれば、そのまま教えてください（手順書を直します）。

## 全体の流れ

| 段階 | 必要なもの | やること | 危険 |
|---|---|---|---|
| **0（この文書）** | Pi、SD カード、電源、ネットワーク | OS を入れる → ビルド → テスト → FOTA のドライラン | なし（何も動かない。システムにも書かない） |
| 1 | + USB メモリ（または SD の空きパーティション） | FOTA のデータボリュームと systemd で、車両アプリを FOTA の管理下で起動する | 低（モーターはつながっていない） |
| 2 | + PiCar-X（Robot HAT） | I2C / GPIO の確認、較正、E-Stop とフェイルセーフの確認 | **あり（車が動く）。タイヤを浮かせて始める** |

段階 1・2 は、この文書の末尾と [fota-pi5-operations.md](../design/fota-pi5-operations.md)、[`rust/sdv-picarx/README.md`](../../rust/sdv-picarx/README.md) に続きます。

## 段階 0: Pi だけで確かめる

### 1. OS を SD カードに書き込む
パソコンで [Raspberry Pi Imager](https://www.raspberrypi.com/software/) を使います。

- **OS は 64-bit を選ぶ**: 「Raspberry Pi OS (other)」→ **Raspberry Pi OS Lite (64-bit)**。画面は使わないので Lite（デスクトップなし）で足ります
  - 32-bit の OS では動きません（このソフトは aarch64 向け）。確認スクリプトが最初に検査します
- 書き込む前の設定（歯車、または「設定を編集する」）で、次を入れておくと、画面とキーボードなしで始められます
  - ホスト名（例: `sdv-pi`）、ユーザー名とパスワード
  - Wi-Fi（有線 LAN ならつなぐだけ）
  - **SSH を有効にする**
- SD カードを Pi に挿して電源を入れ、1〜2 分待つ

### 2. ログインして、道具を入れる
パソコンから（ホスト名とユーザー名は自分で付けたもの）:

```sh
ssh <ユーザー名>@sdv-pi.local
```

Pi の上で:

```sh
sudo apt update && sudo apt full-upgrade -y
sudo apt install -y git build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # Rust（既定のまま Enter）
. "$HOME/.cargo/env"
git clone https://github.com/Hidenori24/SDV.git
cd SDV
```

### 3. 確認スクリプトを実行する

```sh
tools/pi/bringup_check.sh
```

**sudo は付けません**（付けても動きますが、普通のユーザーで実行するのが実機の形に近い）。スクリプトがやることは次のとおりです。

1. **環境の報告**: 機種、OS、64-bit かどうか、glibc の版、メモリ、I2C / GPIO のデバイスの有無
2. **ビルド**: `sdv-fota` と `sdv-picarx`（初回は依存のダウンロードとコンパイルで時間がかかります。所要時間は未計測）
3. **テスト**: 4 つの crate のテストを Pi の上で実行します。**`sdv-picarx` の結合テスト（プロセスを起動して 10 ms の周期の期限を見るもの）は、CI ではエミュレーションのためコンパイルしかしておらず、本物の aarch64 で動かすのはここが初めて**です
4. **FOTA のドライラン**: 一時ディレクトリの中だけで、署名 → 導入 → 試験起動 → health OK で commit、改ざんしたパッケージの拒否、ダウングレードの拒否、health 不合格でロールバック、を通します
5. **デバイスの一覧**（`sdv-picarx-discover`）: I2C と GPIO を読むだけで、ピンには触れません

**やらないこと**: システムに何も入れません（`/etc`・`/var`・systemd に触れない）。書くのはリポジトリの `target/` と、一時ディレクトリ 1 つだけです。GPIO の線や I2C のデバイスに書き込まず、**アクチュエータは一切動かしません**。ドライランで作る署名鍵は使い捨てのテスト用で、一時ディレクトリの中にしかありません。

最後に、こういう一覧が出ます（下は x86-64 の Linux で実行した例）。

```
== summary  (unknown (not a Raspberry Pi, or no device tree), Ubuntu 24.04.5 LTS, 20beff8)
  INFO     0s  not a Pi (x86_64): the checks still run, but this is not the real target
  PASS     0s  build sdv-fota
  PASS     7s  build sdv-picarx
  PASS     5s  test sdv-sec
  PASS     7s  test sdv-core
  PASS     6s  test sdv-fota
  PASS    12s  test sdv-picarx
  PASS     2s  FOTA dry run
  PASS     0s  sdv-picarx-discover
total: 39s, logs: /tmp/sdv-bringup.sRTsYC/logs
RESULT: OK
```

- `tools/pi/bringup_check.sh --quick` は、テストを飛ばしてビルドと FOTA のドライランだけを行います（2 回目以降の確認用）
- **`== summary` から下を、そのまま貼って教えてください。** `FAIL` があれば、表示されたログのファイルも見ます。実機で初めて見つかる不具合（[#174](https://github.com/Hidenori24/SDV/issues/174) のようなアーキテクチャ依存のもの）が出る可能性は十分あります。それを見つけるのがこの段階の目的です

### うまくいかないとき
| 症状 | 原因と対処 |
|---|---|
| `64-bit aarch64 system` が FAIL | 32-bit の OS が入っています。64-bit の OS を書き込み直してください |
| `toolchain` が FAIL | `cargo` か C コンパイラがありません。上の「道具を入れる」をやり直してください（`. "$HOME/.cargo/env"` を忘れていないか） |
| ビルドの途中で止まる・Pi が固まる | メモリ不足の可能性があります（メモリの少ない機種）。`CARGO_BUILD_JOBS=2 tools/pi/bringup_check.sh` で並列数を減らしてください |
| `test sdv-picarx` が FAIL | 期限に間に合わなかった可能性があります（実機でしか分からない情報です）。もう一度実行して、毎回同じテストが落ちるかを教えてください |
| `i2c devices: (none ...)` | I2C が無効です。車体をつなぐ前に `sudo raspi-config nonint do_i2c 0`（または `sudo raspi-config` → Interface Options → I2C）で有効にします。段階 0 では不要です |

## CI が作るバイナリを使う場合の注意（glibc）
GitHub Actions の `aarch64` ジョブは、ビルド済みのバイナリ（成果物 `sdv-aarch64-binaries`）を保存しています。ただし **glibc 2.39 以上の OS でしか起動できません**（[#187](https://github.com/Hidenori24/SDV/issues/187)）。Debian 13（trixie）系の Raspberry Pi OS は 2.41 なので動く見込み、Debian 12（bookworm）系は 2.36 なので起動できない見込みです（どちらも実機では未確認）。確認スクリプトが glibc の版と、使えるかどうかを表示します。**Pi の上でビルドする（この手順）場合は関係ありません。**

## 次の段階（予告）

### 段階 1: FOTA の管理下で車両アプリを起動する（モーターなし）
- FOTA の状態とイメージは `/var/lib/sdv-fota` に置き、**OS とは別のファイルシステム**であることを起動時に確かめます（データボリュームが無いときに、OS の領域へ黙って書かないため）。systemd の mount ユニットは **ラベル `SDV_FOTA` の ext4** を探します
- いちばん簡単なのは **USB メモリを 1 本、ラベル `SDV_FOTA` の ext4 にする**方法です（SD カードのパーティションを切り直さなくて済みます）。SD カードに空きパーティションを作る方法もありますが、Raspberry Pi OS は初回起動時にルート領域を SD カードいっぱいに広げるので、後から縮める手間がかかります
- **フォーマットは中身を消します。** `lsblk` で対象のデバイスを必ず確かめてから行います。手順は [fota-pi5-operations.md](../design/fota-pi5-operations.md) の「Allocate the SDV data volume」以降です
- 署名鍵（seed）は Pi に置きません。パッケージの署名は別のパソコンで行い、Pi には公開鍵だけを置きます

### 段階 2: 車体をつなぐ（ここから先は車が動く）
- 最初は**タイヤを浮かせる**。すぐ電源を切れるようにしておく
- `sdv-picarx-discover` で I2C のデバイスと GPIO のチップ名・ライン番号を確かめ、設定ファイル（`rust/sdv-picarx/config/pi5.disabled.example.json` が出発点。出力が無効の設定）に書く
- モーターの極性、PWM の最小・最大、ステアの中立と左右の端を測って較正する（[#160](https://github.com/Hidenori24/SDV/issues/160)）
- 車両の状態（速度など）を供給する入力ピアが必要です（[#161](https://github.com/Hidenori24/SDV/issues/161)。まだありません）
- **Pi が止まったとき、Robot HAT が最後の PWM を出し続ける**可能性があります。ソフトのフェイルセーフだけに頼らず、電源を切れる手段を用意します
