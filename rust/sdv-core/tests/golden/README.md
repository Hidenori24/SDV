# golden ログ

C++ 実装（`sdv_sim`）の出力を基準にして、Rust 実装が一致することを確認します。

| ファイル | 生成コマンド |
|---|---|
| `cpp_latest.csv` | `./build/sdv_sim` |
| `cpp_stall_brake2.csv` | `./build/sdv_sim --stall brake:2` |
| `cpp_stall_steering3_brake3.5.csv` | `./build/sdv_sim --stall steering:3 --stall brake:3.5` |
| `cpp_stall_engine1_vd2.csv` | `./build/sdv_sim --stall engine:1 --stall vehicledynamics:2` |

いずれも実行後の `logs/latest.csv` をコピーして更新します。CI（`golden-log`）が C++ の出力との一致を確認します。
