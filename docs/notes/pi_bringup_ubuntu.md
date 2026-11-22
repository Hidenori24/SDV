# Ubuntu で最初からやり直す（スマホで SD に書く → SDV を入れる）

パソコンを使わずに、**Android のスマホだけで** Ubuntu Server を microSD に書き込み、Raspberry Pi 5 に SDV を入れて確認スクリプトを動かすまでの手順です。Raspberry Pi OS で進める手順は [pi_bringup.md](pi_bringup.md) です。

> **この手順は、まだ誰も通していません。** Ubuntu・EtchDroid・cloud-init の公式の情報をもとに書いています（末尾の「参考」）。つまずいた箇所があれば、そのまま教えてください（手順書を直します）。

ここまでで**車体は動きません**。確認スクリプトは I2C と GPIO を読むだけで、モーターやサーボには触れません。

## 全体の流れ

| 段階 | どこで | やること |
|---|---|---|
| 1 | スマホ | Ubuntu Server のイメージをダウンロードして、解凍する |
| 2 | スマホ | EtchDroid で microSD に書き込む |
| 3 | スマホ | 初回起動の設定（ユーザー、SSH、ネットワーク）を書く |
| 4 | Pi 5 → スマホ | Pi 5 を起動して、スマホから SSH で入る |
| 5 | Pi 5 | Ubuntu の状態を確かめる（EEPROM、I2C、デバイスの権限） |
| 6 | Pi 5 | SDV を入れて、確認スクリプトを動かす |

## 0. 用意するもの

| もの | メモ |
|---|---|
| Android のスマホ | USB OTG（USB の機器をつなげる）に対応していること。空き容量は数 GB 以上（圧縮イメージ約 1.1GB ＋ 解凍後のイメージ） |
| USB の microSD カードリーダー | スマホにつなげるもの（USB-C、または変換アダプタ）。**スマホ本体の SD スロットには書けません**（EtchDroid の対応外） |
| microSD カード | 16GB 以上。**中身はすべて消えます** |
| Raspberry Pi 5 と電源 | 電源は Pi 5 用の USB-C 電源 |
| LAN ケーブル | **有線をおすすめします**。ルーターにつなぐ。Wi-Fi でもできますが、初回のつまずきが減ります |
| （あると安心）モニターとキーボード | SSH でつながらないときの確認用。Pi 5 のモニター出力は micro HDMI |

- **SunFounder のソフトを入れた SD は残しておく**のがおすすめです。モーターやサーボの動作確認（メーカーのサンプル）は Raspberry Pi OS が前提です。Ubuntu 用には別の SD を使います。
- **iPhone の場合**は、この手順の 1〜3 の代わりに、末尾の「スマホで書けないとき（ネットワークインストール）」を使ってください。

## 1. イメージをダウンロードして、解凍する（スマホ）

1. スマホのブラウザで https://ubuntu.com/download/raspberry-pi を開きます
2. **Ubuntu 26.04 LTS の「Download Ubuntu Server」**を選びます（Desktop ではなく Server。画面は使わないので Server で足ります）
   - ファイル名は `ubuntu-26.04…-preinstalled-server-arm64+raspi.img.xz` のような形です。`arm64+raspi` と `preinstalled-server` が入っていることを確かめます
   - `live-server-arm64.iso` は**使いません**（Raspberry Pi では起動しません）
3. 解凍アプリ（例: **ZArchiver**。XZ の解凍に対応）で、ダウンロードした `.img.xz` を開き、「ここに解凍」します。`.img` のファイルができます
   - EtchDroid は Raspberry Pi のイメージを書けますが、**先に解凍しておく必要があります**

## 2. microSD に書き込む（スマホ）

1. **EtchDroid** を入れます（F-Droid または Google Play）
2. カードリーダーに microSD を挿して、スマホにつなぎます
3. EtchDroid で、イメージを書き込む項目を選び、1 で作った `.img` を選びます
4. 書き込み先にカードリーダーを選び、書き込みます。USB へのアクセスの許可を聞かれたら許可します
5. 書き込みのあとの検証（verify）まで終わるのを待ちます。途中でリーダーを抜かない

## 3. 初回起動の設定を書く（スマホ）

Ubuntu のイメージは、初回起動時に **cloud-init** が設定を読みます。設定ファイルは、SD の最初のパーティション（FAT。名前は `system-boot`）の**直下**にある `user-data` と `network-config` です。

1. 書き込みが終わったら、カードリーダーを一度抜いて、挿し直します
2. スマホのファイルアプリで、USB ストレージ（`system-boot`）が見えることを確かめます
3. テキストエディタのアプリで `user-data` を開き、**中身をすべて次に置き換えます**。`<ユーザー名>` と `<初回だけ使う仮パスワード>` は自分で決めます

```yaml
#cloud-config
hostname: sdv-pi

users:
  - name: <ユーザー名>
    # dialout: Ubuntu では /dev/gpiochip* と /dev/i2c-* がこのグループに開いている
    groups: [adm, sudo, dialout]
    shell: /bin/bash
    lock_passwd: false

# 初回ログインでパスワードの変更を求める
chpasswd:
  expire: true
  users:
    - name: <ユーザー名>
      password: <初回だけ使う仮パスワード>
      type: text

# SSH のパスワードでのログインを許可する（スマホから入るため）
ssh_pwauth: true

# 初回起動のときに入れておく（ネットワークが必要）
package_update: true
packages:
  - avahi-daemon   # sdv-pi.local の名前で見つけられるようにする
  - git
  - build-essential
  - curl
```

- **`dialout` を忘れない**: Ubuntu Server for Raspberry Pi では、GPIO・I2C・SPI のデバイスは `dialout` グループのメンバーが使えます。既定のユーザー（`ubuntu`）は自動で入りますが、自分でユーザーを作るときは自分で入れます。入っていないと、確認スクリプトや車両アプリがデバイスを開けません
- 仮パスワードは SD の FAT 領域に平文で残ります。初回ログインで必ず変わりますが、気になるなら起動後に `/boot/firmware/user-data` から消してください

4. **有線 LAN でつなぐなら、`network-config` は変えなくて大丈夫です**（既定で有線が DHCP になっています）
5. **Wi-Fi を使うなら**、`network-config` を次のようにします

```yaml
version: 2
ethernets:
  eth0:
    dhcp4: true
    optional: true
wifis:
  wlan0:
    dhcp4: true
    optional: false
    access-points:
      "<SSID>":
        password: "<Wi-Fi のパスワード>"
```

- `optional: false` にすると、Wi-Fi がつながるまで初回の設定が待ちます。つながらないと、パッケージのインストールなどが進みません
- Wi-Fi のパスワードも FAT 領域に平文で残ります

> **ファイルアプリで `system-boot` が見えない・編集できないとき**: 3 は飛ばしてかまいません。有線 LAN でつなぎ、既定のユーザー `ubuntu`（初期パスワード `ubuntu`。初回ログインで変更を求められる）で入ります。SSH でパスワードが通らないときは、モニターとキーボードでログインします。この場合、5 の「デバイスの権限」と 6 の `apt install` を必ず行ってください。

6. ファイルアプリで取り外し（アンマウント）をしてから、リーダーを抜きます

## 4. Pi 5 を起動して、スマホから SSH で入る

1. microSD を Pi 5 に挿し、LAN ケーブルをつないでから、電源を入れます
2. **数分待ちます**。初回は、SD いっぱいへの領域の拡張、cloud-init の設定、パッケージのインストールがあり、ふだんより時間がかかります
3. スマホに SSH のアプリを入れます（例: **Termux** に `pkg install openssh`）
4. 入ります

```sh
ssh <ユーザー名>@sdv-pi.local
```

- `sdv-pi.local` で見つからないときは、ルーターの管理画面（またはアプリ）で `sdv-pi` の IP アドレスを調べて、`ssh <ユーザー名>@<IPアドレス>` で入ります。スマホの SSH アプリによっては `.local` の名前を引けません
- 初回ログインで、パスワードの変更を求められます。仮パスワードを入れてから、新しいパスワードを 2 回入れます。そのあと、もう一度 SSH で入り直します

> **起動しない・ネットワークにつながらないとき**: Ubuntu 26.04 LTS は、Pi 5 の**ブート EEPROM の日付が 2025-02-11 以降**であることを求めます。古い EEPROM のままの Pi 5 では起動しないことがあります。モニターをつないで様子を見て、起動しない場合は 5 の「EEPROM が古いとき」を見てください。

## 5. Ubuntu の状態を確かめる（Pi 5）

### 初回の設定が終わったか

```sh
cloud-init status --wait     # 「status: done」になれば終わり
```

`error` のときは `sudo cat /var/log/cloud-init-output.log` の最後のほうを見ます。パッケージのインストールに失敗していたら、6 の `apt install` で入れ直せば大丈夫です。

### EEPROM の日付

```sh
sudo rpi-eeprom-update
```

`CURRENT:` の日付を控えます。今動いているなら問題ありません。

- **EEPROM が古いとき**（26.04 が起動しないとき）: Ubuntu **24.04 LTS** のイメージ（EEPROM の日付の要件がありません）を同じ手順で別の SD に書いて起動し、`sudo rpi-eeprom-update -a` を実行して再起動します。もう一度 `sudo rpi-eeprom-update` で日付が新しくなったことを確かめてから、26.04 の SD に戻します

### I2C が有効か

```sh
ls -l /dev/i2c-* /dev/gpiochip*
grep -n i2c_arm /boot/firmware/config.txt
```

- Ubuntu Server for Raspberry Pi は、**既定で I2C が有効**です（`config.txt` に `dtparam=i2c_arm=on`）。Raspberry Pi OS の `raspi-config` は Ubuntu にはありません
- `/dev/i2c-1` が無いときは、`/boot/firmware/config.txt` の `[all]` の下に `dtparam=i2c_arm=on` があるかを確かめ、無ければ足して再起動します
- 確認スクリプトの表示にある「enable I2C with raspi-config」は Raspberry Pi OS 向けの案内です。Ubuntu では上の確認をしてください

### デバイスの権限

```sh
id                                   # groups に dialout があること
ls -l /dev/gpiochip* /dev/i2c-*      # グループが dialout で、rw になっていること
```

- `id` に `dialout` が無いときは、`sudo usermod -aG dialout $USER` を実行して、SSH で入り直します
- デバイスのグループが `dialout` でないとき（Ubuntu の版で変わった場合など）は、`ls -l` の結果をそのまま教えてください

## 6. SDV を入れて、確認スクリプトを動かす（Pi 5）

```sh
sudo apt update && sudo apt full-upgrade -y
sudo apt install -y git build-essential curl          # 3 で入れていれば何も起きない
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # Rust（既定のまま Enter）
. "$HOME/.cargo/env"
git clone https://github.com/Hidenori24/SDV.git
cd SDV
tools/pi/bringup_check.sh
```

- **sudo は付けません**
- 初回のビルドは、依存のダウンロードとコンパイルで時間がかかります（未計測）。SSH が切れても止まらないようにするなら、`tmux` か `screen` の中で動かします（`sudo apt install -y tmux` → `tmux` → その中で実行）
- スクリプトがやること・やらないこと、`FAIL` のときの見方は [pi_bringup.md の「3. 確認スクリプトを実行する」](pi_bringup.md#3-確認スクリプトを実行する) と同じです
- **`== summary` から下を、そのまま貼って教えてください。** `test sdv-picarx` は、10 ms の周期の期限を本物の aarch64 で初めて測るテストです

## Raspberry Pi OS 版との違い

| 項目 | Raspberry Pi OS（[pi_bringup.md](pi_bringup.md)） | Ubuntu（この手順） |
|---|---|---|
| SD への書き込み | パソコンの Raspberry Pi Imager | スマホの EtchDroid（解凍してから） |
| 初回の設定 | Imager の設定画面 | `user-data` / `network-config`（cloud-init） |
| I2C の有効化 | `raspi-config` | 既定で有効（`/boot/firmware/config.txt`） |
| GPIO・I2C の権限 | Raspberry Pi OS のグループ | `dialout` グループ |
| CI のビルド済みバイナリ | Trixie 系なら使える見込み（glibc 2.41） | 確認スクリプトが glibc の版と使えるかを表示する |
| SunFounder のソフト | 説明書の前提 | 前提外。メーカーのサンプルは Raspberry Pi OS の SD で動かす |

## うまくいかないとき

| 症状 | 原因と対処 |
|---|---|
| EtchDroid にカードリーダーが出てこない | スマホが USB OTG に対応しているか、リーダーに SD が挿さっているかを確かめる。スマホ本体の SD スロットは使えない |
| EtchDroid がイメージを書けない | `.img.xz` のまま選んでいないか。解凍した `.img` を選ぶ |
| `system-boot` が見えない | 3 を飛ばして、既定のユーザー `ubuntu` で入る（3 の囲みを参照） |
| `sdv-pi.local` で入れない | ルーターで IP アドレスを調べて、IP で入る。`avahi-daemon` が入るのは初回の設定が終わってから |
| SSH で `Permission denied` | `user-data` の `ssh_pwauth: true` と、ユーザー名・仮パスワードを確かめる |
| 26.04 が起動しない | EEPROM の日付が 2025-02-11 より古い可能性。5 の「EEPROM が古いとき」 |
| 確認スクリプトの I2C / GPIO の一覧で、デバイスを開けない | `id` に `dialout` があるか。無ければ `sudo usermod -aG dialout $USER` → 入り直す |
| ビルドの途中で止まる・Pi が固まる | メモリ不足の可能性。`CARGO_BUILD_JOBS=2 tools/pi/bringup_check.sh` |

## スマホで書けないとき（ネットワークインストール）

iPhone の場合や、スマホで書き込めないときは、Pi 5 だけで Ubuntu を入れられます。**LAN ケーブル、USB キーボード、モニター**が必要です。

1. 空の microSD を Pi 5 に挿し、LAN ケーブル・キーボード・モニターをつないで電源を入れます
2. 起動画面で **Shift を押し続け**ます。ブートローダーが、Raspberry Pi Imager（組み込み版）をネットワークからダウンロードして起動します
3. Imager で、OS に **Ubuntu Server 26.04 LTS（64-bit）**、書き込み先に microSD を選びます。設定画面で、ホスト名（`sdv-pi`）、ユーザー名とパスワード、SSH を入れておきます
4. 書き込みが終わったら再起動し、4 の「スマホから SSH で入る」へ進みます。**このときも 5 の「デバイスの権限」を確かめてください**（Imager で作ったユーザーが `dialout` に入っているとは限りません）

Ubuntu の公式手順は「参考」のリンクにあります。

## 参考

- [Install Ubuntu on a Raspberry Pi（ダウンロード）](https://ubuntu.com/download/raspberry-pi)
- [Install Ubuntu on the Raspberry Pi（対応機種と、最小の EEPROM の日付）](https://ubuntu.com/hardware/docs/boards/how-to/ubuntu_supported/raspberry-pi/)
- [Configure your board for headless use（cloud-init の seed）](https://ubuntu.com/hardware/docs/boards/how-to/special_hardware/headless-usage/)
- [Edit the Raspberry Pi boot configuration（Ubuntu の config.txt の既定）](https://ubuntu.com/hardware/docs/boards/how-to/special_hardware/rpi-config-txt/)
- [Install Ubuntu on Raspberry Pi directly（ネットワークインストール）](https://documentation.ubuntu.com/hardware-support/boards/tutorials/raspberry-pi-solo/)
- [EtchDroid（F-Droid）](https://f-droid.org/packages/eu.depau.etchdroid/)
- [GPIO・I2C の権限を dialout に開いた件（Launchpad）](https://bugs.launchpad.net/bugs/1880627)
