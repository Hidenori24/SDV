"""Figure 1: internal architecture of every node (VCU, Engine/Brake/Steering ECU, Plant)."""
from svgkit import Svg, C, tw

COLS = [
    ("VCU / ゲートウェイ ECU", "ecu", {
        "swc": ["DriverInputSWC", "　シナリオ → DriverInput（20ms）", "SafetySupervisorSWC", "　Watchdog（100ms）"],
        "model": ["safety_logic", "　UpdateHeartbeat", "　ComputeSafetyState"],
        "rte": ["input / safety / state", "alive_rx[4]（各ノードの", "　生存カウンタ）"],
        "bsw": ["Com: Safety・DriverInput を送信", "　Alive・VehicleState を受信", "Diag: 無応答時間を数える"],
        "io_tx": ["Safety（10ms）", "DriverInput（20ms）"],
        "io_rx": ["Alive ×4（10ms）", "VehicleState（10ms）"],
    }),
    ("Engine ECU", "ecu", {
        "swc": ["EngineSWC（10ms）", "　throttle → drive_accel_cmd"],
        "model": ["engine_model", "　ComputeDriveAccel"],
        "rte": ["input / safety", "cmd.drive_accel_cmd", "rx_age（受信経過）"],
        "bsw": ["Failsafe: 入力途絶 → 入力0", "　Safety途絶 → 自律EStop", "Diag: ReportAlive"],
        "io_tx": ["CmdDrive（直結 → Plant）", "Alive（10ms）"],
        "io_rx": ["DriverInput（20ms）", "Safety（10ms）"],
    }),
    ("Brake ECU", "ecu", {
        "swc": ["BrakeSWC（10ms）", "　brake → brake_decel_cmd"],
        "model": ["brake_model", "　ComputeBrakeDecel", "　（EStop → 最大減速）"],
        "rte": ["input / safety", "cmd.brake_decel_cmd", "rx_age（受信経過）"],
        "bsw": ["Failsafe: 入力途絶 → 入力0", "　Safety途絶 → 自律EStop", "Diag: ReportAlive"],
        "io_tx": ["CmdBrake（直結 → Plant）", "Alive（10ms）"],
        "io_rx": ["DriverInput（20ms）", "Safety（10ms）"],
    }),
    ("Steering ECU", "ecu", {
        "swc": ["SteeringSWC（10ms）", "　steer → steer_angle_cmd", "　（舵角の状態を保持）"],
        "model": ["steering_model", "　ComputeSteerTarget", "　StepSteeringDynamics", "　（一次遅れ）"],
        "rte": ["input / safety", "cmd.steer_angle_cmd", "rx_age（受信経過）"],
        "bsw": ["Failsafe: 入力途絶 → 入力0", "　Safety途絶 → 自律EStop", "Diag: ReportAlive"],
        "io_tx": ["CmdSteer（直結 → Plant）", "Alive（10ms）"],
        "io_rx": ["DriverInput（20ms）", "Safety（10ms）"],
    }),
    ("Plant（ECUではない）", "plant", {
        "swc": ["VehicleDynamicsSWC", "　（10ms）", "　縦 → 横 の順に更新"],
        "model": ["vehicledynamics_model", "　StepLongitudinal", "　StepLateral", "　StepVehicle"],
        "rte": ["cmd（3つ）/ safety", "state（t, x, y, yaw,", "　v, yaw_rate, wheel_omega）"],
        "bsw": ["（ECUではないので", "　フェイルセーフなし）", "Diag: ReportAlive"],
        "io_tx": ["VehicleState（10ms）", "Alive（10ms）"],
        "io_rx": ["Cmd ×3（直結）", "Safety（直結）"],
    }),
]

ROWS = [
    ("swc", "SWC", "Application 層。RTE を読み、Model を呼び、RTE に書く"),
    ("model", "Model", "純粋関数。RTE に依存しない"),
    ("rte", "RTE", "ECU ごとの信号の入れ物"),
    ("bsw", "BSW", "ECU 内の基盤サービス"),
    ("io_rx", "受信", "他ノードから届くフレーム"),
    ("io_tx", "送信", "他ノードへ送るフレーム"),
]
ROW_H = {"swc": 92, "model": 96, "rte": 78, "bsw": 78, "io_rx": 62, "io_tx": 62}
ROW_STYLE = {"swc": "swc", "model": "model", "rte": "rte", "bsw": "bsw", "io_rx": "io", "io_tx": "io"}


def build():
    LABEL_W, COL_W, GAP, X0, Y0 = 138, 262, 12, 24, 110
    W = X0 + LABEL_W + len(COLS) * (COL_W + GAP) + 12
    H = Y0 + sum(ROW_H.values()) + len(ROWS) * 10 + 84
    s = Svg(W, H, "ECUごとの内部アーキテクチャ",
            "VCU・Engine ECU・Brake ECU・Steering ECU・Plant それぞれの SWC / Model / RTE / BSW とフレームの送受信")
    s.text(W / 2, 40, "ECUごとの内部アーキテクチャ", 24, "700", "middle")
    s.text(W / 2, 66, "各ノードは同じ 4 層（SWC・Model・RTE・BSW）を持ち、RTE は ECU ごとに 1 つ。他ノードとはフレームでやり取りする", 14, "400", "middle", C["sub"])

    # column headers
    for i, (name, kind, _) in enumerate(COLS):
        x = X0 + LABEL_W + i * (COL_W + GAP)
        f, st = C[kind]
        s.rect(x, 80, COL_W, 36, f, st, 2, 8)
        s.text(x + COL_W / 2, 104, name, 16, "700", "middle")

    y = Y0 + 12
    for key, label, hint in ROWS:
        h = ROW_H[key]
        f, st = C[ROW_STYLE[key]]
        # row label
        s.rect(X0, y, LABEL_W - 8, h, f, st, 1.5, 8)
        s.text(X0 + (LABEL_W - 8) / 2, y + h / 2 - 2, label, 16, "700", "middle", st)
        s.lines(X0 + (LABEL_W - 8) / 2, y + h / 2 + 16, [], 10)
        for i, (_, kind, content) in enumerate(COLS):
            x = X0 + LABEL_W + i * (COL_W + GAP)
            s.rect(x, y, COL_W, h, f, st, 1.2, 8)
            items = content[key]
            lh = 17.5
            top = y + (h - len(items) * lh) / 2 + 13
            for j, it in enumerate(items):
                bold = key in ("swc", "model") and not it.startswith("　")
                s.text(x + 12, top + j * lh, it, 12.5, "700" if bold else "400", "start", C["ink"] if bold or key in ("rte", "bsw", "io_rx", "io_tx") else C["sub"])
        y += h + 10

    # legend / notes
    ly = y + 14
    s.text(X0, ly, "読み方", 13, "700", "start", C["sub"])
    s.text(X0 + 56, ly, "SWC は渡された RTE だけを読み書きする（他ノードの内部は見ない）／ ECU 間は「受信 → RTE」「RTE → 送信」だけでつながる／ 直結はバス障害（遅延・欠損・断）の影響を受けない", 13, "400", "start", C["sub"])
    s.text(X0 + 56, ly + 22, "Failsafe: DriverInput が 200ms 超途絶 → 入力を0扱い、Safety が 100ms 超途絶 → そのECUが自律的にEStop相当（フレームが戻れば自動復帰）", 13, "400", "start", C["sub"])
    return s.render()


if __name__ == "__main__":
    import sys
    open(sys.argv[1], "w", encoding="utf-8").write(build())
