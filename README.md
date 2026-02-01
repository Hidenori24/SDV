# SDV Playground (AUTOSAR-like, C++ Core)

本プロジェクトは、個人開発で「SDV（Software-Defined Vehicle）を自作していく」ための最小アーキテクチャと実装ベースを提供します。  
最終的にはRCカー相当の構造へ載せ替え可能な設計とし、初期は完全シミュレータ（2D）で開発・検証します。

## 目的

- AUTOSAR（Classic寄り）を参考に、以下の構造を最小限で成立させる
  - SWC（アプリケーション部品）分割
  - RTE相当の信号I/F固定（Read/Write）
  - BSW相当（時間基盤、通信、NVM、診断/監視、ログ）
- “細かく”段階を刻みながら、部品単体→統合→SDV化（更新/監視/ログ/分割）へ到達する

## 最初のスコープ（v1）

- シミュレータ（2D）で「アクセル/ブレーキ/ハンドル」で車が動く
- 車輪回転（wheel_omega）を模擬してログに出る
- E-Stop（安全停止）が必ず効く
- パラメータ更新（Config/NVM）で挙動が変わる（例：最大速度、最大舵角、最大減速度）
- 同一入力でリプレイできる（ログ再現性）

## アーキテクチャ概要（AUTOSAR-like）

- Application Layer（SWC）
  - DriverInputSWC / EngineSWC / BrakeSWC / SteeringSWC / VehicleDynamicsSWC / SafetySupervisorSWC
- RTE
  - Rte_Read / Rte_Write と Runnable 呼び出し（手書き最小実装）
- BSW
  - TimeBase / Com / Nvm(Config) / Diag+Watchdog / Logging
- Platform
  - 依存（OS、I/O、時刻、ファイル等）の抽象化

## ドキュメント

- docs/design/00_overview.md : 全体方針と用語
- docs/design/01_architecture.md : 全体設計（層、データフロー）
- docs/design/02_interfaces_signals.md : 信号I/F定義
- docs/design/03_components.md : 部品設計（SWC/BSW/RTE）
- docs/design/04_tasks_scheduling.md : タスク設計（周期、順序）
- docs/design/05_safety_logging_update.md : 安全/ログ/更新設計
- docs/design/06_roadmap_dod.md : 作成順序、成果物、完成条件（DoD）
- docs/adr/ADR-0001-autosar-like-architecture.md : 重要設計判断（ADR）

## ビルド・実行（雛形）

要件:
- CMake 3.20+
- C++17

ビルド例:
- mkdir -p build
- cmake -S . -B build
- cmake --build build -j

実行例:
- ./build/sdv_sim

実行すると `build/logs/latest.csv` にログが出ます（雛形）。

## データ可視化

シミュレーション実行後、Python 可視化ツールで結果をグラフ表示できます：

```bash
# 初回のみ：Python 仮想環境とパッケージをインストール
python3 -m venv venv_viz
source venv_viz/bin/activate
pip install matplotlib numpy

# 可視化実行（PNG/PDF 生成）
python3 tools/python/plot_integration_test.py
```

出力ファイル：
- `docs/test_reports/plots/engine_brake_integration_plot.png` (1967x1397px, 152KB)
- `docs/test_reports/plots/engine_brake_integration_plot.pdf` (37KB)

グラフ内容：
- 📊 ドライバー入力（アクセル・ブレーキ）
- ⚙️ アクチュエータコマンド（エンジン・ブレーキ）
- 🔗 相関分析（入出力の線形性確認）
- 🚗 車速の時間推移
- 🗺️ 車両軌跡（X-Y 平面）

## 開発の進め方（重要）

本PJは「最初に全部を作らない」方針です。

1. 骨格固定（Signals/RTE/TimeBase/Logging）
2. Engineのみ（加速と車輪回転）
3. Steeringのみ（ヨー更新）
4. Brakeのみ（減速）
5. 統合（縦→横→Pose）
6. SDV化（監視、更新、通信分割）

## ライセンス

- LICENSEを参照（MIT）
