# Task: VehicleDynamics SWC 実装タスク一覧

## フェーズ分け

```
Phase 1: Model層追加実装・テスト
    → Task-1: StepLateral() を Model層に追加実装
    → Task-2: Model層テスト (test_vehicledynamics_model.cpp) を拡張
    → Task-3: 全テスト PASS確認

Phase 2: SWC層リファクタリング（既に Model::StepLongitudinal は呼び出し中）
    → Task-4: VehicleDynamicsSWC で StepLateral() を追加呼び出し
    → Task-5: SWC層テスト (test_vehicledynamics_swc.cpp) を実装
    → Task-6: 全テスト PASS確認

Phase 3: 統合確認
    → Task-7: リグレッション確認
```

---

## タスク詳細

### Task-1: StepLateral() を Model層に追加実装

**ドキュメント参照**: [docs/design/vehicledynamics_swc.md#3.2](../../docs/design/vehicledynamics_swc.md#32-steplateral-関数仕様未実装)

**ファイル**: `src/model/vehicledynamics_model.h`

**実装内容**:
```cpp
namespace Model {

inline VehicleState StepLateral(
    const VehicleState& s,
    float dt,
    float steer_angle_cmd,
    const VehicleParams& p)
{
    VehicleState out = s;
    
    // ヨーレート計算：yaw_rate = v / L × tan(δ)
    float L = std::max(p.wheelbase_m, 1e-4f);
    float yaw_rate = (out.v / L) * std::tan(steer_angle_cmd);
    
    // ヨー角更新
    out.yaw += yaw_rate * dt;
    
    // 位置更新
    out.x += out.v * std::cos(out.yaw) * dt;
    out.y += out.v * std::sin(out.yaw) * dt;
    
    return out;
}

}
```

**完了条件**:
- [ ] ファイルが修正される
- [ ] コンパイルエラーなし

---

### Task-2: Model層テスト (test_vehicledynamics_model.cpp) を拡張

**ドキュメント参照**: [docs/design/vehicledynamics_swc.md#5.1](../../docs/design/vehicledynamics_swc.md#51-model層テスト)

**ファイル**: `tests/test_vehicle_dynamics_model.cpp`

**テストケース（追加）**:
1. `steer_angle_cmd=0` → yaw は変わらない
2. `steer_angle_cmd > 0` → yaw_rate > 0（右旋回）
3. `v=0 のとき` → yaw_rate = 0（静止時は旋回なし）
4. `x, y が更新される`（位置変化を確認）
5. `steer_angle_cmd < 0` → yaw_rate < 0（左旋回）

**完了条件**:
- [ ] コンパイルエラーなし
- [ ] 5つ以上のテストケースが実装される

---

### Task-3: 全テスト PASS確認

**内容**:
```bash
cd /Users/matsumoto/work/SDV/build
cmake --build .
ctest --verbose
```

**完了条件**:
- [ ] `test_vehicle_dynamics_model` PASS
- [ ] ビルドエラーなし

---

### Task-4: VehicleDynamicsSWC で StepLateral() を追加呼び出し

**ドキュメント参照**: [docs/design/vehicledynamics_swc.md#4.1](../../docs/design/vehicledynamics_swc.md#41-vehicledynamicsswcmain10ms)

**ファイル**: `src/swc/vehicledynamics_swc.cpp`

**修正内容**:
現在のコード：
```cpp
auto next_state = Model::StepLongitudinal(...);
```

修正後：
```cpp
auto next_state = Model::StepLongitudinal(...);
next_state = Model::StepLateral(next_state, dt, steer_angle_cmd, params);
```

**完了条件**:
- [ ] コンパイルエラーなし

---

### Task-5: SWC層テスト (test_vehicledynamics_swc.cpp) を実装

**ドキュメント参照**: [docs/design/vehicledynamics_swc.md#5.2](../../docs/design/vehicledynamics_swc.md#52-swc層テスト)

**ファイル**: `tests/test_vehicledynamics_swc.cpp`

**テストケース**:
1. 複数ステップ実行後、速度が増加
2. ブレーキ後、速度が減少
3. ハンドル切ると、yaw が変わる
4. E-Stop で強制的に減速
5. 位置（x, y）が更新される

**完了条件**:
- [ ] コンパイルエラーなし
- [ ] 5つ以上のテストケースが実装される

---

### Task-6: 全テスト PASS確認

**内容**:
```bash
ctest --verbose
```

**チェックリスト**:
- [ ] `test_vehicle_dynamics_model` PASS
- [ ] `test_vehicledynamics_swc` PASS

---

### Task-7: リグレッション確認

**内容**:
1. 既存シミュレーション実行
2. ログ出力に速度・位置・姿勢がある
3. 値に異常がない（物理的に合理的な軌跡）

**完了条件**:
- [ ] シミュレーション成功
- [ ] ログ確認完了

---

## 進捗管理

| Task | Status | 開始日 | 完了日 |
|------|--------|--------|--------|
| Task-1 | ⏳ 未開始 | - | - |
| Task-2 | ⏳ 未開始 | - | - |
| Task-3 | ⏳ 未開始 | - | - |
| Task-4 | ⏳ 未開始 | - | - |
| Task-5 | ⏳ 未開始 | - | - |
| Task-6 | ⏳ 未開始 | - | - |
| Task-7 | ⏳ 未開始 | - | - |
