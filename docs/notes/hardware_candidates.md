# 実機（Raspberry Pi の車）の検討メモ

**採用: SunFounder PiCar-X + Raspberry Pi 5**（実機での動作確認はこれから）。
最初のFOTA対象は、Pi上で車両全体を動かす一つの`vehicle`アプリとする。論理ECUを分割してもよいが、物理ECUや個別更新を前提にしない。ハードウェアI/OはPlatform APIの下に置く。

## 前提
- 計算機は Raspberry Pi 5、**64bit OS**（Raspberry Pi OS Lite 64bit）
- Pi上の一つの車両制御アプリが実車を動かす。仮想ECUやプロセス分割は必要に応じて導入できるが、初期のOTAはアプリ全体を一度に更新する
- **既製品の完成車**を使いたい。今のモデル（前輪操舵のバイシクルモデル）に合う、前輪操舵（アッカーマン）の車を選ぶ

## 候補（2026-10 時点で調べた内容。価格・在庫・販売店は未確認）
| 候補 | 特徴 | 合うところ | 注意 |
|---|---|---|---|
| **Waveshare PiRacer（Standard / Pro）** | アッカーマン操舵。Standard は後輪 2 モーター駆動、Pro は Donkey Car 公式が組み立て済みの車として紹介。拡張基板に 5V 降圧回路と PCA9685 が 2 つ（モーター用・サーボ用）。電池は 18650×3（Pro は 4 セル） | 前輪操舵でモデルと一致。実機に近い大きさ・速度 | **対象は Raspberry Pi 4。Pi 5 対応は未確認**。Pi と電池は別売り。在庫が不安定な販売店がある |
| **SunFounder PiCar-X** | アッカーマン操舵（前輪サーボ）。Robot HAT が PWM と電源を担当。Pi 5/4/3B+/Zero 2W 対応と販売ページにある | 安くて小型。最初の一台として低リスク | 速度は低い。エンコーダーによる閉ループ制御はない |
| Hiwonder MentorPi A1 | Pi 5、アッカーマン、エンコーダー付きモーター、LiDAR、ROS2 対応 | 速度センサが最初からある | ROS2 前提で、今の独自スタックと文化が違う。高価 |
| Donkey Car（自作に近い） | 市販の RC カー + Pi + PCA9685 + カメラ | 情報が多い | 組み立てが必要で「既製品」ではない |

対象外: 4 輪の差動駆動のキット（Freenove 4WD など）。操舵の方式が違い、今のモデルに合わない。

## 残っている実機作業
- Robot HATのI2C/GPIOをLinux Platform実装へ接続し、PiCar-Xで確認する
- 速度センサのない構成で開ループ制御を評価する
- 実機での操作入口、車両寸法、最大舵角、速度上限を決めてConfigへ反映する

## 実機に向けてソフトでやること（部品が決まってから）
- Linux 用の Platform の実装: I2C/GPIOでRobot HATを制御する
  - 付属の Python ライブラリは使わない。ハードの I2C の仕様だけを使う
- アクチュエータ指令（加速度・舵角）から PWM への変換と、較正（キャリブレーション）の表
- 入力: 今の GUI を Pi から配信、またはゲームパッド（evdev）
- 実機の寸法・最大舵角・最高速度を Config（`vehicle.*`、`steering.*`）に反映して、シミュレータを実車に合わせる

## 安全（キットを使うときも変わらない）
- PCA9685 は、Pi が止まっても**最後の PWM を出し続ける**。キットの電源スイッチだけでは足りない
  - Pi が止まったら電源を切る仕組み（GPIO 制御のリレーで ESC 電源を入れる、など）か、フェイルセーフを持つマイコンを足す
- **物理的なキルスイッチ**で、モーターへの電源を切れること
- 最初の試験は**タイヤを浮かせた状態**で行い、Config で速度上限を低くする
- ソフト側のフェイルセーフ（受信途絶で自律的に止まる）が実機でも効くことを、試験で確認する

## 情報源
- [PiRacer AI Kit（welectron）](https://www.welectron.com/Waveshare-17674-PiRacer-AI-Kit-Acce_1)
- [PiRacer Pro AI Kit（Waveshare Wiki）](https://waveshare.com/wiki/PiRacer_Pro_AI_Kit)
- [PiRacer Pro（RobotShop）](https://www.robotshop.com/products/piracer-pro-high-speed-ai-racing-robot-w-o-raspberry-pi)
- [PiRacer DonkeyCar（Botland）](https://botland.com.pl/en/waveshare-educational-robots/16403-piracer-donkeycar-4-wheel-robot-platform-with-camera-dc-drive-and-oled-display-for-raspberry-pi.html)
- [How to Build a Donkey（Donkey Car 公式）](https://docs.donkeycar.com/guide/build_hardware/)
- [Donkey Car FAQ](https://docs.donkeycar.com/support/faq/)
- [SunFounder PiCar-X のドキュメント](https://docs.sunfounder.com/projects/picar-x-v20/en/latest/python/python_move.html)
- [Best Raspberry Pi Robot Kits（microcontrollerslab）](https://microcontrollerslab.com/best-raspberry-pi-robot-kits-for-beginners-students-makers-buying-guide/)
- [Hiwonder MentorPi A1 レビュー（ThinkRobotics）](https://thinkrobotics.com/blogs/product-reviews-buying-guides/hiwonder-mentorpi-a1-review-raspberry-pi-5-robot-car-with-ackermann-chassis-and-ros2)
