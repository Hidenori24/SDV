# Task-2: Mock RTE 作成

## 概要

テスト用に RTE をモック化した `mock_rte.h` を作成します。

## 関連ドキュメント

- 設計書: [docs/design/engine_swc.md](../../docs/design/engine_swc.md)
- タスク一覧: [README.md](./README.md)

## 実装内容

### ファイル場所
```
tests/mock_rte.h
```

### 責務

- SWC テストで RTE の入出力を**グローバル変数で管理**
- Read/Write 関数を Mock 化
- Signal の値をテスト中に自由に設定・検証

### 実装方針

1. **グローバル変数**で Signal を保持
   ```cpp
   static Rte::DriverInput g_driver_input{};
   static Rte::Safety g_safety{};
   static Rte::ActuatorCmd g_actuator_cmd{};
   ```

2. **Mock 関数**で Read/Write をフック
   ```cpp
   Rte::DriverInput Mock_Rte_Read_DriverInput() {
       return g_driver_input;
   }
   ```

3. **Helper 関数**でテストから値を設定
   ```cpp
   void Mock_Rte_SetDriverInput(const Rte::DriverInput& in) {
       g_driver_input = in;
   }
   ```

## テスト側での使用例

```cpp
#include "mock_rte.h"

TEST_CASE("Engine: throttle 0.5 with estop=false") {
    // 1. Mock RTE に値をセット
    Rte::DriverInput driver_in{{throttle=0.5f, ...}};
    Rte::Safety safety{{estop=false, ...}};
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // 2. SWC を実行
    Swc::Engine::Main10ms(0.01);
    
    // 3. 出力を確認
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.0f, 0.01f));
}
```

## 完了条件

- [ ] `tests/mock_rte.h` が作成される
- [ ] コンパイルエラーなし
- [ ] Test から `#include "mock_rte.h"` が通る

## 次のステップ

- Task-3 で実際のテストケースを実装
