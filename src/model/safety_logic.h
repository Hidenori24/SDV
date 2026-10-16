#pragma once
#include <cstdint>

namespace Model {

/**
 * @brief Safety state (values match Rte::SystemState)
 */
enum class SafetyState : uint8_t {
    Normal = 0,
    Degraded = 1,
    EStop = 2
};

struct SafetyParams {
    uint32_t heartbeat_timeout_ms = 1000; ///< a monitored SWC is "dead" when silent for longer than this
};

/**
 * @brief Per-SWC liveness verdict (true = alive)
 */
struct HeartbeatStatus {
    bool engine_ok = true;
    bool brake_ok = true;
    bool steering_ok = true;
    bool vehicledynamics_ok = true;
};

/**
 * @brief Watchdog state for one monitored SWC
 */
struct HeartbeatMonitor {
    uint64_t last_count = 0; ///< alive counter observed at the previous check
    uint32_t silent_ms = 0;  ///< time during which the counter did not advance
};

/**
 * @brief Update one monitor with the current alive counter (pure apart from `m`)
 *
 * The counter advancing resets the silence timer. Otherwise the silence grows by dt_ms.
 *
 * @return true while the SWC is considered alive (silent_ms <= timeout)
 */
inline bool UpdateHeartbeat(HeartbeatMonitor& m, uint64_t alive_count, uint32_t dt_ms, const SafetyParams& p)
{
    if (alive_count != m.last_count) {
        m.last_count = alive_count;
        m.silent_ms = 0;
    } else {
        m.silent_ms += dt_ms;
    }
    return m.silent_ms <= p.heartbeat_timeout_ms;
}

inline int CountFailed(const HeartbeatStatus& hb)
{
    return (hb.engine_ok ? 0 : 1) + (hb.brake_ok ? 0 : 1) +
           (hb.steering_ok ? 0 : 1) + (hb.vehicledynamics_ok ? 0 : 1);
}

/**
 * @brief 0 dead -> Normal, 1 dead -> Degraded, 2 or more dead -> EStop
 */
inline SafetyState ComputeSafetyState(const HeartbeatStatus& hb)
{
    const int failed = CountFailed(hb);
    if (failed >= 2) return SafetyState::EStop;
    if (failed == 1) return SafetyState::Degraded;
    return SafetyState::Normal;
}

} // namespace Model
