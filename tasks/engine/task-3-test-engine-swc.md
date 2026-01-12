# Task-3: SWC層テスト (test_engine_swc.cpp) を作成

## 概要

Mock RTE を使用した SWC 層テストを実装します。

## 関連ドキュメント

- 設計書: [docs/design/engine_swc.md#52](../../docs/design/engine_swc.md#52-swc層テスト)
- タスク一覧: [README.md](./README.md)
- Task-2: [task-2-mock-rte.md](./task-2-mock-rte.md)

## 実装内容

### ファイル場所
```
tests/test_engine_swc.cpp
```

### テストケース

#### ケース 1: 正常系 (throttle=0.5, estop=false)
```cpp
TEST_CASE("EngineSWC: throttle 0.5 produces accel 1.0") {
    // Setup
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    
    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // Execute
    Swc::Engine::Main10ms(0.01);
    
    // Verify
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(1.0f, 0.01f));
}
```

#### ケース 2: E-Stop (estop=true)
```cpp
TEST_CASE("EngineSWC: estop forces zero accel") {
    // Setup
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.0f;  // 最大でも
    
    Rte::Safety safety{};
    safety.estop = true;
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // Execute
    Swc::Engine::Main10ms(0.01);
    
    // Verify
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}
```

#### ケース 3: Clamp (throttle > 1.0)
```cpp
TEST_CASE("EngineSWC: throttle > 1.0 is clamped to max accel") {
    // Setup
    Rte::DriverInput driver_in{};
    driver_in.throttle = 1.5f;  // > 1.0
    
    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::Normal;
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // Execute
    Swc::Engine::Main10ms(0.01);
    
    // Verify
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE_THAT(cmd.drive_accel_cmd, WithinAbs(2.0f, 0.01f));
}
```

#### ケース 4: System State EStop
```cpp
TEST_CASE("EngineSWC: system_state EStop forces zero accel") {
    // Setup
    Rte::DriverInput driver_in{};
    driver_in.throttle = 0.5f;
    
    Rte::Safety safety{};
    safety.estop = false;
    safety.system_state = Rte::SystemState::EStop;  // ← system state
    
    Mock_Rte_SetDriverInput(driver_in);
    Mock_Rte_SetSafety(safety);
    
    // Execute
    Swc::Engine::Main10ms(0.01);
    
    // Verify
    auto cmd = Mock_Rte_Written_ActuatorCmd();
    REQUIRE(cmd.drive_accel_cmd == 0.0f);
}
```

## ファイル構成

```cpp
#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_floating_point.hpp>
#include "mock_rte.h"
#include "swc/engine_swc.h"

using Catch::Matchers::WithinAbs;

// ケース 1, 2, 3, 4 を実装...
```

## 完了条件

- [ ] `tests/test_engine_swc.cpp` が作成される
- [ ] 4つ以上のテストケースが実装される
- [ ] コンパイルエラーなし
- [ ] 全テストケースが PASS

## 次のステップ

- Task-4: 全テスト PASS 確認
