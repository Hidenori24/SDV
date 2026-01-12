# Model層 実装仕様書

## 1. Engine Model 実装仕様

### Header: src/model/engine_model.h

```cpp
#pragma once
#include <algorithm>

namespace Model {

/**
 * @struct EngineParams
 * @brief エンジン制御パラメータ
 */
struct EngineParams {
    float max_accel_mps2 = 2.0f;  ///< 最大加速度 (m/s²)
};

/**
 * @brief 駆動加速度指令を計算する純粋関数
 * 
 * @param throttle_0_1 スロットル入力 (0.0～1.0, 正規化値)
 * @param estop 緊急停止フラグ
 * @param p パラメータ
 * @return 駆動加速度指令 (m/s²)
 * 
 * @detail
 * estop = true の場合は常に 0.0 を返す。
 * estop = false の場合、throttle を max_accel_mps2 でスケーリング。
 * throttle は 0.0～1.0 に自動クランプ。
 */
inline float ComputeDriveAccel(
    float throttle_0_1,
    bool estop,
    const EngineParams& p)
{
    if (estop) {
        return 0.0f;
    }
    
    float th = std::clamp(throttle_0_1, 0.0f, 1.0f);
    return th * p.max_accel_mps2;
}

} // namespace Model
```

**テスト仕様**:
| throttle | estop | 期待値 | 説明 |
|----------|-------|--------|------|
| 0.0 | false | 0.0 m/s² | 最小 |
| 0.5 | false | 1.0 m/s² | 中程度 |
| 1.0 | false | 2.0 m/s² | 最大 |
| 1.5 | false | 2.0 m/s² | clamp |
| -0.5 | false | 0.0 m/s² | clamp |
| 0.5 | true | 0.0 m/s² | e-stop override |
| 1.0 | true | 0.0 m/s² | e-stop override |

---

## 2. Brake Model 実装仕様

### Header: src/model/brake_model.h

```cpp
#pragma once
#include <algorithm>

namespace Model {

/**
 * @struct BrakeParams
 * @brief ブレーキ制御パラメータ
 */
struct BrakeParams {
    float max_decel_mps2 = 4.0f;           ///< 最大制動減速度 (m/s²)
    float estop_max_decel_mps2 = 4.0f;     ///< e-stop時の強制減速度 (m/s²)
};

/**
 * @brief 制動減速度指令を計算する純粋関数
 * 
 * @param brake_0_1 ブレーキ入力 (0.0～1.0, 正規化値)
 * @param estop 緊急停止フラグ
 * @param p パラメータ
 * @return 制動減速度指令 (m/s²，正の値)
 * 
 * @detail
 * estop = true の場合は最大減速度を返す。
 * estop = false の場合、brake を max_decel_mps2 でスケーリング。
 */
inline float ComputeBrakeDecel(
    float brake_0_1,
    bool estop,
    const BrakeParams& p)
{
    if (estop) {
        return p.estop_max_decel_mps2;
    }
    
    float b = std::clamp(brake_0_1, 0.0f, 1.0f);
    return b * p.max_decel_mps2;
}

} // namespace Model
```

**テスト仕様**:
| brake | estop | 期待値 | 説明 |
|-------|-------|--------|------|
| 0.0 | false | 0.0 m/s² | 最小 |
| 0.5 | false | 2.0 m/s² | 中程度 |
| 1.0 | false | 4.0 m/s² | 最大 |
| 1.5 | false | 4.0 m/s² | clamp |
| 0.5 | true | 4.0 m/s² | e-stop override |

---

## 3. Steering Model 実装仕様

### Header: src/model/steering_model.h

```cpp
#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

/**
 * @struct SteeringParams
 * @brief ステアリング制御パラメータ
 */
struct SteeringParams {
    float max_steer_angle_rad = 0.40f;     ///< 最大舵角 (rad, ±0.4)
    float time_constant_s = 0.15f;         ///< 一次遅れ時定数 (s)
};

/**
 * @brief ステアリングダイナミクス（一次遅れ）を計算する純粋関数
 * 
 * @param current_angle 現在の舵角 (rad)
 * @param target_angle 目標舵角 (rad)
 * @param time_constant_s 時定数 τ (s)
 * @param dt_s ステップ時間 (s)
 * @param p パラメータ (使用：max_steer_angle_rad でclamp)
 * @return 次のステップでの舵角 (rad)
 * 
 * @detail
 * 一次遅れフィルタの実装：
 *   α = 1 - exp(-dt / τ)
 *   y(t+dt) = y(t) + α × (target - y(t))
 * 
 * target は ±max_steer_angle_rad でclampされる。
 */
inline float StepSteeringDynamics(
    float current_angle,
    float target_angle,
    float time_constant_s,
    float dt_s,
    const SteeringParams& p)
{
    // Target clamp
    float max_angle = p.max_steer_angle_rad;
    float target = std::clamp(target_angle, -max_angle, max_angle);
    
    // First-order lag: α = 1 - exp(-dt / τ)
    float tau = std::max(time_constant_s, 1e-4f);  // Avoid division by zero
    float alpha = 1.0f - std::exp(-dt_s / tau);
    
    // Update: new = current + α(target - current)
    float new_angle = current_angle + alpha * (target - current_angle);
    
    return new_angle;
}

} // namespace Model
```

**テスト仕様**:
| current | target | τ | dt | 期待値 | 説明 |
|---------|--------|---|----|----|------|
| 0.0 | 0.4 | 0.15 | 0.01 | ~0.026 | 1ステップ進展 |
| 0.0 | 0.4 | 0.15 | 0.15 | ~0.253 | τ時に63%到達 |
| 0.4 | 0.0 | 0.15 | 0.01 | ~0.374 | 減少 |
| 0.0 | 0.5 | 0.15 | 0.01 | ~0.026 | target clamp to 0.4 |
| 0.0 | 0.0 | 0.15 | 0.01 | 0.0 | no change |

---

## 4. VehicleDynamics Model 実装仕様

### Header: src/model/vehicledynamics_model.h

```cpp
#pragma once
#include <algorithm>
#include <cmath>

namespace Model {

/**
 * @struct VehicleParams
 * @brief 車両ダイナミクスパラメータ
 */
struct VehicleParams {
    float wheel_radius_m = 0.03f;          ///< 車輪半径 (m)
    float wheelbase_m = 0.20f;             ///< ホイールベース (m)
    float linear_drag = 0.0f;              ///< 線形抵抗係数
    float max_speed_mps = 3.0f;            ///< 最大速度 (m/s)
    float estop_decel_mps2 = 6.0f;         ///< e-stop時の追加減速 (m/s²)
};

/**
 * @struct VehicleState
 * @brief 車両状態
 */
struct VehicleState {
    float t = 0.0f;              ///< 時刻 (s)
    float v = 0.0f;              ///< 速度（縦） (m/s)
    float wheel_omega = 0.0f;     ///< 車輪角速度 (rad/s)
    float x = 0.0f;              ///< 位置 x (m)
    float y = 0.0f;              ///< 位置 y (m)
    float yaw = 0.0f;            ///< ヨー角 (rad)
    float yaw_rate = 0.0f;       ///< ヨーレート (rad/s)
};

/**
 * @brief 抵抗を計算
 */
inline float Resist(float v, const VehicleParams& p) {
    return p.linear_drag * v;
}

/**
 * @brief 縦方向ダイナミクス（速度更新）
 * 
 * @param s 現在の状態
 * @param dt ステップ時間 (s)
 * @param drive_accel_cmd 駆動加速度 (m/s²)
 * @param brake_decel_cmd 制動減速度 (m/s²)
 * @param estop 緊急停止フラグ
 * @param p パラメータ
 * @return 次の状態（速度・車輪速度更新）
 * 
 * @detail
 * 速度更新式：
 *   accel = drive_accel - brake_decel - drag
 *   if (estop) accel -= estop_decel
 *   v_next = clamp(v + accel × dt, 0, v_max)
 *   wheel_omega = v / r
 */
inline VehicleState StepLongitudinal(
    const VehicleState& s,
    float dt,
    float drive_accel_cmd,
    float brake_decel_cmd,
    bool estop,
    const VehicleParams& p)
{
    VehicleState out = s;
    
    // 加速度計算
    float accel = drive_accel_cmd - brake_decel_cmd - Resist(out.v, p);
    if (estop) {
        accel -= p.estop_decel_mps2;
    }
    
    // 速度更新（0から v_max でclamp）
    out.v = std::clamp(
        out.v + accel * dt,
        0.0f,
        p.max_speed_mps
    );
    
    // 車輪速度更新
    float r = std::max(p.wheel_radius_m, 1e-4f);
    out.wheel_omega = out.v / r;
    
    // 時刻更新
    out.t += dt;
    
    return out;
}

/**
 * @brief 横方向ダイナミクス（位置・姿勢更新）
 * 
 * @param s 現在の状態
 * @param dt ステップ時間 (s)
 * @param steer_angle_cmd 舵角指令 (rad)
 * @param p パラメータ
 * @return 次の状態（位置・姿勢更新）
 * 
 * @detail
 * バイシクルモデル：
 *   yaw_rate = v / L × tan(δ)
 *   yaw_next = yaw + yaw_rate × dt
 *   x_next = x + v × cos(yaw) × dt
 *   y_next = y + v × sin(yaw) × dt
 * 
 * ここで L はホイールベース、δ は舵角
 */
inline VehicleState StepLateral(
    const VehicleState& s,
    float dt,
    float steer_angle_cmd,
    const VehicleParams& p)
{
    VehicleState out = s;
    
    // ホイールベース（ゼロ除算回避）
    float L = std::max(p.wheelbase_m, 1e-4f);
    
    // ヨーレート計算：yaw_rate = v / L × tan(δ)
    float yaw_rate = (out.v / L) * std::tan(steer_angle_cmd);
    
    // ヨー角更新
    out.yaw += yaw_rate * dt;
    
    // 位置更新（Euler method）
    out.x += out.v * std::cos(out.yaw) * dt;
    out.y += out.v * std::sin(out.yaw) * dt;
    
    return out;
}

} // namespace Model
```

**テスト仕様 (StepLongitudinal)**:
| v | drive | brake | estop | dt | 期待値 | 説明 |
|---|-------|-------|-------|----|----|------|
| 0.0 | 2.0 | 0.0 | false | 0.01 | 0.02 m/s | 加速 |
| 1.0 | 0.0 | 0.5 | false | 0.01 | 0.995 m/s | 減速 |
| 0.1 | 0.0 | 100.0 | false | 0.01 | 0.0 m/s | clamp at 0 |
| 2.9 | 1.0 | 0.0 | false | 0.01 | 3.0 m/s | clamp at max |
| 1.0 | 0.0 | 0.0 | true | 0.01 | 0.94 m/s | e-stop decel |

**テスト仕様 (StepLateral)**:
| v | yaw | steer | 期待値 yaw | 期待値 x | 説明 |
|---|-----|-------|-----------|---------|------|
| 0.0 | 0.0 | 0.2 | 0.0 | 0.0 | 静止時は旋回なし |
| 1.0 | 0.0 | 0.0 | 0.0 | 0.01 | 直進 |
| 1.0 | 0.0 | 0.1 | >0.0 | <0.01 | 右旋回 |
| 1.0 | π/4 | 0.0 | π/4 | >0.01 | 前進方向変化 |

---

## 5. Safety Logic 実装仕様（参考）

```cpp
namespace Model {

enum class SafetyState {
    Normal = 0,
    Degraded = 1,
    EStop = 2
};

struct HeartbeatStatus {
    bool engine_ok;
    bool brake_ok;
    bool steering_ok;
    bool vehicledynamics_ok;
};

inline SafetyState ComputeSafetyState(const HeartbeatStatus& hb) {
    int num_failed = 0;
    if (!hb.engine_ok) num_failed++;
    if (!hb.brake_ok) num_failed++;
    if (!hb.steering_ok) num_failed++;
    if (!hb.vehicledynamics_ok) num_failed++;
    
    if (num_failed >= 2) return SafetyState::EStop;
    if (num_failed >= 1) return SafetyState::Degraded;
    return SafetyState::Normal;
}

} // namespace Model
```

---

## 6. 実装チェックリスト

- [ ] すべての Model 関数が pure function（副作用なし）
- [ ] パラメータは `const` で参照渡し
- [ ] 入力値は自動 clamp（不正な値を防止）
- [ ] ゼロ除算回避（1e-4f で下限設定）
- [ ] テストケースが全網羅
- [ ] ドキュメントコメント完備（@brief, @param, @return, @detail）
