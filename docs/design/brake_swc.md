# Brake SWC 設計書

## 1. 概要

**Brake SWC** は、ドライバー入力（brake）を受け取り、制動減速度指令を生成するコンポーネントです。

- **責務**: brake (0..1) → brake_decel_cmd (m/s²)
- **周期**: 10ms
- **階層**: Model層 + SWC層の2層構造

---

## 2. アーキテクチャ

### 2.1 層の責任分離

```
┌─────────────────────────────────────────┐
│ SWC層（BrakeSWC）                       │
│ - RTE から brake, estop を読む          │
│ - Model::ComputeBrakeDecel() を呼ぶ    │
│ - 結果を RTE へ書く                      │
│ - テスト対象：「正しく呼び出しているか」  │
└─────────────────────────────────────────┘
          ↓ (入出力)
┌─────────────────────────────────────────┐
│ Model層（brake_model.h）                │
│ - 純粋関数：ComputeBrakeDecel()          │
│ - RTE非依存                             │
│ - テスト対象：「計算結果が正しいか」      │
└─────────────────────────────────────────┘
```

### 2.2 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | `brake` | float (0..1) | ドライバーの制動指示（正規化値） |
| | `estop` | bool | 緊急停止フラグ |
| **パラメータ** | `max_decel_mps2` | float | 最大制動減速度（デフォルト: 4.0 m/s²） |
| **出力** | `brake_decel_cmd` | float (m/s²) | 制動減速度指令（正の値） |

---

## 3. 計算ロジック（Model層）

### 3.1 ComputeBrakeDecel() 関数仕様

```cpp
float ComputeBrakeDecel(
    float brake_0_1,           // 入力：0..1
    bool estop,                // 入力：緊急停止フラグ
    const BrakeParams& p       // パラメータ
)
```

**計算式**：

```
if (estop):
    brake_decel_cmd = p.estop_max_decel_mps2  (= 4.0 m/s², 最大強制制動)
else:
    brake_clamped = clamp(brake_0_1, 0.0, 1.0)
    brake_decel_cmd = brake_clamped × p.max_decel_mps2
```

### 3.2 要件

- ✅ brake は 0.0～1.0 の範囲に **クランプ**
- ✅ estop = true のとき、常に最大制動 (4.0 m/s²) を返す
- ✅ 副作用なし（pure function）
- ✅ RTE 非依存

---

## 4. SWC層仕様

### 4.1 BrakeSWC::Main10ms()

**責務**：
1. RTE から `DriverInput` を読む
2. RTE から `Safety` を読む（estop 判定用）
3. Model::ComputeBrakeDecel() を呼ぶ
4. 結果を RTE の `ActuatorCmd.brake_decel_cmd` へ書く

---

## 5. テスト戦略

### 5.1 Model層テスト（test_brake_model.cpp）

**テストケース**：
1. `brake=0, estop=false` → `decel=0`
2. `brake=0.5, estop=false` → `decel=2.0`
3. `brake=1.0, estop=false` → `decel=4.0`
4. `estop=true` → `decel=4.0` (最大制動)
5. `brake > 1.0` → `decel=4.0` (clamp)

**ステータス**: ❌ **未実装**

### 5.2 SWC層テスト（test_brake_swc.cpp）

**テストケース**：
1. `brake=0.5, estop=false` → `decel=2.0`
2. `estop=true` → `decel=4.0`
3. `brake > 1.0` → clamp確認

**ステータス**: ❌ **未実装**

---

## 6. 完了条件

- [ ] Model層（brake_model.h）実装・テスト完了
- [ ] SWC層 (brake_swc.cpp) Model呼び出しに置き換え
- [ ] SWC層テスト実装
- [ ] 全テスト PASS
- [ ] リグレッション確認
