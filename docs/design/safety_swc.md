# Safety SWC 設計書

## 1. 概要

**Safety SWC** は、システムの安全状態を監視し、ハートビート監視と E-Stop 制御を行うコンポーネントです。

- **責務**: ハートビート監視、E-Stop フラグ管理、システム状態の遷移
- **周期**: 100ms（低速監視タスク）
- **重要性**: 安全機能（機能安全に対応可能な設計）

---

## 2. アーキテクチャ

### 2.1 責務分離

```
┌──────────────────────────────────────────┐
│ SWC層（SafetySupervisorSWC）             │
│ - ハートビート監視ロジック               │
│ - システム状態の遷移管理                 │
│ - E-Stop フラグの生成・伝播              │
│ - テスト対象：「状態遷移が正しいか」      │
└──────────────────────────────────────────┘
```

---

## 3. 入出力仕様

| 項目 | 値 | 型 | 説明 |
|------|----|----|------|
| **入力** | `heartbeat_status` | struct | 各SWCのハートビート情報 |
| **パラメータ** | `hb_timeout_ms` | uint32_t | ハートビートタイムアウト閾値（1000 ms） |
| **出力** | `estop` | bool | 緊急停止フラグ |
| | `system_state` | enum | システム状態（Normal / Degraded / EStop） |

---

## 4. ロジック

### 4.1 ハートビート監視

```
各SWC（Engine, Brake, Steering, VehicleDynamics）に対して：
  - 前フレームとの時刻差をチェック
  - dt > timeout_ms → 「SWC応答なし」と判定
  - 1つでも応答なし → system_state = Degraded
  - 複数の応答なし → system_state = EStop
```

### 4.2 状態遷移

```
      [Normal]
         ↑ ↓
   1個未応答
      [Degraded]
         ↓
   2個以上未応答
      [EStop]
```

### 4.3 E-Stop フラグ

```
estop = true when:
  - system_state == EStop, OR
  - 外部 E-Stop 信号 (UI/ハードウェア)
```

---

## 5. テスト戦略

### 5.1 Model層テスト（test_safety_logic.cpp）

**テストケース**：
1. すべてのハートビートが正常 → `system_state = Normal`
2. 1つのSWCが応答なし → `system_state = Degraded`
3. 複数SWCが応答なし → `system_state = EStop`

**ステータス**: ❌ **未実装**

### 5.2 SWC層テスト（test_safety_swc.cpp）

**テストケース**：
1. 初期状態で `estop=false, system_state=Normal`
2. SWCのハートビートが途絶すると状態遷移
3. 外部 E-Stop 信号で即座に `estop=true`

**ステータス**: ❌ **未実装**

---

## 6. 完了条件

- [ ] ハートビート監視ロジック実装
- [ ] 状態遷移ロジック実装
- [ ] Model層テスト実装
- [ ] SWC層テスト実装
- [ ] 全テスト PASS
- [ ] リグレッション確認

**優先度**: 低（Engine / Brake / Steering 完了後に実装予定）
