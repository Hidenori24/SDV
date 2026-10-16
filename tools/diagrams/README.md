# tools/diagrams

`docs/design/img/` の SVG 図を生成するスクリプト（Python 3 標準ライブラリのみ）。

```bash
python3 tools/diagrams/gen_all.py
```

| 図 | スクリプト | 出力 |
|---|---|---|
| ECU ごとの内部アーキテクチャ | `fig_ecu_internal.py` | `docs/design/img/ecu_internal.svg` |
| ECU 間のアーキテクチャ | `fig_ecu_network.py` | `docs/design/img/ecu_network.svg` |
| C++ と Rust の関係 | `fig_cpp_rust.py` | `docs/design/img/cpp_rust.svg` |

- 共通の描画部品は `svgkit.py`（箱・矢印・ラベル）。図は SVG なので、生成せずに直接編集してもよい（その場合は次に再生成すると上書きされる点に注意）
- 各 SVG は自前の白い背景を持つので、ダークテーマのページでも読める
- 構成を変えたら（ECU・フレーム・モジュールの追加など）、該当のスクリプトを直して再生成する
