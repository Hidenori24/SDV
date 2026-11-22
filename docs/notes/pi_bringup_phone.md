# パソコンなしで、スマホから SD に書く（Raspberry Pi OS）

[pi_bringup.md](pi_bringup.md) の「1. OS を SD カードに書き込む」を、パソコンの Raspberry Pi Imager の代わりに **Android のスマホだけで**行う手順です。SD に Raspberry Pi OS Lite (64-bit) を書き、初回起動の設定（ユーザー、SSH、I2C、ネットワーク）をして、スマホから SSH で入るところまでを扱います。そのあとは pi_bringup.md の「2. ログインして、道具を入れる」に戻ります。

> **この手順は、まだ誰も通していません。** Raspberry Pi・EtchDroid・cloud-init の公式の情報をもとに書いています（末尾の「参考」）。つまずいた箇所があれば、そのまま教えてください（手順書を直します）。

ここまでで**車体は動きません**。

## 全体の流れ

| 段階 | どこで | やること |
|---|---|---|
| 1 | スマホ | Raspberry Pi OS Lite (64-bit) のイメージをダウンロードして、解凍する |
| 2 | スマホ | EtchDroid で microSD に書き込む |
| 3 | スマホ | 初回起動の設定（`user-data` / `network-config`）を書く |
| 4 | Pi 5 → スマホ | Pi 5 を起動して、スマホから SSH で入る |
| 5 | Pi 5 | 初回の設定が効いたかを確かめて、pi_bringup.md に戻る |

## 0. 用意するもの

| もの | メモ |
|---|---|
| Android のスマホ | USB OTG（USB の機器をつなげる）に対応していること。空き容量は数 GB 以上（圧縮イメージ ＋ 解凍後のイメージ） |
| USB の microSD カードリーダー | スマホにつなげるもの（USB-C、または変換アダプタ）。**スマホ本体の SD スロットには書けません**（EtchDroid の対応外） |
| microSD カード | **中身はすべて消えます**。SunFounder のソフトを入れた SD とは別の SD を使うのがおすすめです（pi_bringup.md の「PiCar-X の組み立て」） |
| Raspberry Pi 5 と電源 | Pi 5 用の USB-C 電源 |
| LAN ケーブル | **有線をおすすめします**。Wi-Fi でもできますが、初回のつまずきが減ります |
| （あると安心）モニターとキーボード | SSH でつながらないときの確認用。Pi 5 のモニター出力は micro HDMI |

**iPhone の場合**は、末尾の「スマホで書けないとき（ネットワークインストール）」を使ってください。

## 1. イメージをダウンロードして、解凍する（スマホ）

1. スマホのブラウザで https://www.raspberrypi.com/software/operating-systems/ を開きます
2. **Raspberry Pi OS (64-bit) の「Raspberry Pi OS Lite」**をダウンロードします
   - Debian 13（Trixie）ベースのものを選びます。ファイル名に `trixie`、`arm64`、`lite` が入っていることを確かめます（例: `…-raspios-trixie-arm64-lite.img.xz`）
   - 「Legacy」（Bookworm ベース）は選びません（理由は pi_bringup.md の「glibc」の節）。また、Bookworm には次の 3 で使う cloud-init が入っていません
3. 解凍アプリ（例: **ZArchiver**。XZ の解凍に対応）で、ダウンロードした `.img.xz` を開き、「ここに解凍」します。`.img` のファイルができます
   - EtchDroid は Raspberry Pi のイメージを書けますが、**先に解凍しておく必要があります**

## 2. microSD に書き込む（スマホ）

1. **EtchDroid** を入れます（F-Droid または Google Play）
2. カードリーダーに microSD を挿して、スマホにつなぎます
3. EtchDroid で、イメージを書き込む項目を選び、1 で作った `.img` を選びます
4. 書き込み先にカードリーダーを選び、書き込みます。USB へのアクセスの許可を聞かれたら許可します
5. 書き込みのあとの検証（verify）まで終わるのを待ちます。途中でリーダーを抜かない

## 3. 初回起動の設定を書く（スマホ）

Trixie ベースの Raspberry Pi OS は、初回起動時に **cloud-init** が設定を読みます。設定ファイルは、SD の最初のパーティション（FAT。名前は `bootfs`）の**直下**にある `user-data` と `network-config` です（`meta-data` はそのままで構いません）。パソコンの Imager が設定画面で作っているのも、このファイルです。

1. 書き込みが終わったら、カードリーダーを一度抜いて、挿し直します
2. スマホのファイルアプリで、USB ストレージ（`bootfs`）が見えることを確かめます
3. テキストエディタのアプリで `user-data` を開き、**中身をすべて次に置き換えます**。`<ユーザー名>` と `<パスワード>` は自分で決めます

```yaml
#cloud-config
hostname: sdv-pi
manage_etc_hosts: true
timezone: Asia/Tokyo

users:
  - name: <ユーザー名>
    # gpio / i2c: /dev/gpiochip* と /dev/i2c-* を、sudo なしで使うため
    groups: users,adm,sudo,gpio,i2c,spi,netdev
    shell: /bin/bash
    lock_passwd: false
    plain_text_passwd: "<パスワード>"

# Raspberry Pi OS 独自の設定。I2C を有効にする（raspi-config の I2C と同じ）
rpi:
  i2c: true

# SSH を有効にする
enable_ssh: true
ssh_pwauth: true
```

- **1 行目の `#cloud-config` は消さないでください。** これが無いと、cloud-init は設定を読みません
- YAML なので、**字下げ（スペースの数）が意味を持ちます**。上の例の字下げのまま書きます。タブは使いません
- **`gpio` と `i2c` を忘れない**: Raspberry Pi OS では、GPIO と I2C のデバイスはこのグループに開いています。入っていないと、確認スクリプトや車両アプリがデバイスを開けません
- `rpi:` の `i2c: true` で I2C を有効にしておくと、pi_bringup.md の `raspi-config` の手順が要らなくなります
- パスワードは SD の FAT 領域に平文で残ります。気になるなら、起動して SSH で入ったあとに `passwd` で変えてください

4. **有線 LAN でつなぐなら、`network-config` は変えなくて大丈夫です**
5. **Wi-Fi を使うなら**、`network-config` の中身を次に置き換えます

```yaml
network:
  version: 2
  wifis:
    renderer: NetworkManager
    wlan0:
      dhcp4: true
      regulatory-domain: "JP"
      access-points:
        "<SSID>":
          password: "<Wi-Fi のパスワード>"
      optional: true
```

- `regulatory-domain` は使う国のコードにします（日本は `JP`）
- Wi-Fi のパスワードも FAT 領域に平文で残ります

6. ファイルアプリで取り外し（アンマウント）をしてから、リーダーを抜きます

> **ファイルアプリで `bootfs` が見えない・編集できないとき**: スマホだけではユーザーを作れません。モニターとキーボードをつないで起動すると、初回にユーザーを作る画面が出るはずです（未確認）。それも難しいときは、末尾の「スマホで書けないとき」を使ってください。

## 4. Pi 5 を起動して、スマホから SSH で入る

1. microSD を Pi 5 に挿し、LAN ケーブルをつないでから、電源を入れます
2. **数分待ちます**。初回は、SD いっぱいへの領域の拡張と、cloud-init の設定で、ふだんより時間がかかります（途中で 1 回再起動することがあります）
3. スマホに SSH のアプリを入れます（例: **Termux** に `pkg install openssh`）
4. 入ります

```sh
ssh <ユーザー名>@sdv-pi.local
```

- `sdv-pi.local` で見つからないときは、ルーターの管理画面（またはアプリ）で `sdv-pi` の IP アドレスを調べて、`ssh <ユーザー名>@<IPアドレス>` で入ります。スマホの SSH アプリによっては `.local` の名前を引けません

## 5. 設定が効いたかを確かめる（Pi 5）

```sh
cloud-init status --wait      # 「status: done」になれば終わり
id                            # groups に gpio と i2c があること
ls -l /dev/i2c-* /dev/gpiochip*
```

- `/dev/i2c-1` があれば、I2C は有効です
- `cloud-init status` が `error` のときは、`sudo cat /var/log/cloud-init-output.log` の最後のほうを見ます
- `/dev/i2c-1` が無いときは、pi_bringup.md の「うまくいかないとき」のとおり `sudo raspi-config nonint do_i2c 0` で有効にします
- `id` に `gpio` や `i2c` が無いときは、`sudo usermod -aG gpio,i2c $USER` を実行して、SSH で入り直します

ここまでできたら、[pi_bringup.md の「2. ログインして、道具を入れる」](pi_bringup.md#2-ログインして道具を入れる)に戻ります（`ssh` はもう済んでいるので、`sudo apt update` から）。

## うまくいかないとき

| 症状 | 原因と対処 |
|---|---|
| EtchDroid にカードリーダーが出てこない | スマホが USB OTG に対応しているか、リーダーに SD が挿さっているかを確かめる。スマホ本体の SD スロットは使えない |
| EtchDroid がイメージを書けない | `.img.xz` のまま選んでいないか。解凍した `.img` を選ぶ |
| `bootfs` に `user-data` が無い | Bookworm（Legacy）を書いていないか。Trixie の 64-bit Lite を書き直す |
| SSH がつながらない（connection refused） | `user-data` の `enable_ssh: true` と、1 行目の `#cloud-config`、字下げを確かめる。直したら SD を書き直して最初からやる（設定は初回の起動でしか読まれない） |
| SSH で `Permission denied` | ユーザー名とパスワード、`lock_passwd: false` と `ssh_pwauth: true` を確かめる |
| `sdv-pi.local` で入れない | ルーターで IP アドレスを調べて、IP で入る |

## スマホで書けないとき（ネットワークインストール）

iPhone の場合や、スマホで書き込めないときは、Pi 5 だけで OS を入れられます。**LAN ケーブル、USB キーボード、モニター**が必要です。

1. 空の microSD を Pi 5 に挿し、LAN ケーブル・キーボード・モニターをつないで電源を入れます
2. 起動画面で **Shift を押し続け**ます。ブートローダーが、Raspberry Pi Imager（組み込み版）をネットワークからダウンロードして起動します
3. Imager で、OS に **Raspberry Pi OS Lite (64-bit)**、書き込み先に microSD を選びます。設定画面で、ホスト名（`sdv-pi`）、ユーザー名とパスワード、SSH を入れておきます
4. 書き込みが終わったら再起動し、4 の「スマホから SSH で入る」へ進みます。I2C は、pi_bringup.md のとおり `raspi-config` で有効にします

## 参考

- [Raspberry Pi OS のダウンロード](https://www.raspberrypi.com/software/operating-systems/)
- [Cloud-init on Raspberry Pi OS（Raspberry Pi の公式ブログ。`user-data` / `network-config` と `rpi:` の設定）](https://www.raspberrypi.com/news/cloud-init-on-raspberry-pi-os/)
- [EtchDroid（F-Droid）](https://f-droid.org/packages/eu.depau.etchdroid/)
- [Testing Raspberry Pi's new Network Install feature（ネットワークインストール）](https://www.jeffgeerling.com/blog/2022/testing-raspberry-pis-new-network-install-feature/)
