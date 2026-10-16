"""Figure 2: architecture between the nodes: vehicle bus, frames, direct links, fault-injection points."""
from svgkit import Svg, C, tw

W, H = 1400, 960


def node(s, x, y, w, h, title, lines, kind):
    f, st = C[kind]
    s.rect(x, y, w, h, f, st, 2.2, 10)
    s.text(x + w / 2, y + 28, title, 17, "700", "middle")
    s.lines(x + w / 2, y + 52, lines, 12.5, 18, "middle", C["sub"])


def build():
    s = Svg(W, H, "ECU間のアーキテクチャ",
            "VCU・3つのECU・Plant が車載バスと直結線でつながる構成。フレーム、周期、故障注入点、フェイルセーフ")
    s.text(W / 2, 40, "ECU間のアーキテクチャ", 24, "700", "middle")
    s.text(W / 2, 66, "ECU 間は車載バスのフレーム（遅延・欠損・断を注入できる）。Plant とのアクチュエータ線・E-Stop 線は直結で、バス障害の影響を受けない", 14, "400", "middle", C["sub"])

    # --- nodes
    VX, VY, VW, VH = 450, 100, 500, 100
    node(s, VX, VY, VW, VH, "VCU / ゲートウェイ ECU", ["DriverInputSWC（入力）  SafetySupervisorSWC（Watchdog）", "RTE(VCU)"], "ecu")
    ECUS = [(150, "Engine ECU", "EngineSWC"), (550, "Brake ECU", "BrakeSWC"), (950, "Steering ECU", "SteeringSWC")]
    EY, EH, EW = 380, 100, 300
    for x, name, swc in ECUS:
        node(s, x, EY, EW, EH, name, [swc, f"RTE({name.split()[0]})  ＋ フェイルセーフ"], "ecu")
    PX, PY, PW, PH = 450, 700, 500, 100
    node(s, PX, PY, PW, PH, "Plant（ECUではない・制御対象）", ["VehicleDynamicsSWC", "RTE(Plant)"], "plant")

    # --- bus bar
    BY, BH, BX0, BX1 = 270, 38, 110, 1290
    s.rect(BX0, BY, BX1 - BX0, BH, C["bus"][0], C["bus"][1], 2.5, 6)
    s.text((BX0 + BX1) / 2, BY + 25, "車載バス（CAN風・固定周期フレーム・最新値を保持）", 16, "700", "middle", "#166534")

    # --- VCU <-> bus  (down = TX, up = RX)
    s.path(f"M{VX + 130},{VY + VH} V{BY}", "#2563eb", 2.6, marker_end="a_blue")
    s.label(VX + 130, 238, "Safety 10ms・DriverInput 20ms", 12.5, "#1d4ed8", "middle")
    s.path(f"M{VX + VW - 130},{BY} V{VY + VH}", "#d97706", 2.6, marker_end="a_orange")
    s.label(VX + VW - 130, 238, "Alive ×4・VehicleState 10ms", 12.5, "#b45309", "middle")

    # --- bus <-> each ECU
    for x, name, _ in ECUS:
        cx = x + EW / 2
        s.path(f"M{cx - 50},{BY + BH} V{EY}", "#2563eb", 2.6, marker_end="a_blue")
        s.path(f"M{cx + 50},{EY} V{BY + BH}", "#d97706", 2.6, marker_end="a_orange")
        s.label(cx - 50, 340, "DriverInput・Safety", 12, "#1d4ed8", "end", 4)
        s.label(cx + 50, 340, "Alive 10ms", 12, "#b45309", "start", 4)

    # --- direct links ECU -> Plant (thick)
    PL_IN = {150: PX + 70, 550: PX + 250, 950: PX + PW - 70}
    names = {150: "CmdDrive", 550: "CmdBrake", 950: "CmdSteer"}
    for x, name, _ in ECUS:
        cx = x + EW / 2
        tx = PL_IN[x]
        mid = 600 if x != 550 else None
        if x == 550:
            s.path(f"M{cx},{EY + EH} V{PY}", "#374151", 4.5, marker_end="a_dark")
            s.label(cx - 14, 620, "CmdBrake（直結）", 13, "#111827", "end", 5, "#ffffff", "700")
        else:
            s.path(f"M{cx},{EY + EH} V560 H{tx} V{PY}", "#374151", 4.5, marker_end="a_dark")
            s.label((cx + tx) / 2, 552, f"{names[x]}（直結）", 13, "#111827", "middle", 5, "#ffffff", "700")

    # --- VCU -> Plant direct Safety (left route)
    s.path(f"M{VX},{VY + 50} H50 V{PY + 50} H{PX}", "#374151", 4.5, marker_end="a_dark")
    s.vlabel(50, 460, "Safety（直結・E-Stop線）", 13, "#111827", 5, "#ffffff", "700")
    # --- Plant -> bus (right route): VehicleState + Alive (bus link)
    s.path(f"M{PX + PW},{PY + 50} H1350 V{BY + BH / 2} H{BX1}", "#d97706", 2.6, marker_end="a_orange")
    s.vlabel(1350, 560, "VehicleState・Alive 10ms", 12.5, "#b45309", 4)

    # --- fault injection points (numbered)
    s.badge(VX + VW + 16, VY + 16, 2)
    for x, _, _ in ECUS:
        s.badge(x + EW + 16, EY + 16, 1)
    s.badge(BX0 - 22, BY + BH / 2, 3)
    s.badge(PX + PW + 16, PY + 16, 1)

    # --- legend
    LY = 842
    s.rect(30, LY - 24, W - 60, 112, C["panel"], "#d1d5db", 1, 8)
    s.path("M50,%d H110" % (LY - 4), "#2563eb", 2.6, marker_end="a_blue")
    s.text(122, LY, "バス: 下り（VCU → 各ECU）", 13)
    s.path("M50,%d H110" % (LY + 22), "#d97706", 2.6, marker_end="a_orange")
    s.text(122, LY + 26, "バス: 上り（生存・センサ → VCU）", 13)
    s.path("M50,%d H110" % (LY + 48), "#374151", 4.5, marker_end="a_dark")
    s.text(122, LY + 52, "直結（バス障害の影響なし）", 13)

    s.badge(380, LY - 5, 1)
    s.text(398, LY, "ECU / Plant の停止（stall）: 実行もフレーム送信も止まる → Alive が途絶", 13)
    s.badge(380, LY + 21, 2)
    s.text(398, LY + 26, "VCU の停止: Safety・DriverInput が届かなくなる → 各ECUが自律的にフェイルセーフ", 13)
    s.badge(380, LY + 47, 3)
    s.text(398, LY + 52, "バス障害: 遅延（ms）・欠損（%）・断。フレームが戻れば自動で復帰", 13)

    s.text(1010, LY - 4, "フェイルセーフ（各 ECU）", 13, "700")
    s.text(1010, LY + 17, "DriverInput 途絶 > 200ms → 入力0扱い", 12.5, "400", "start", C["sub"])
    s.text(1010, LY + 38, "Safety 途絶 > 100ms → 自律的に EStop 相当", 12.5, "400", "start", C["sub"])
    s.text(1010, LY + 59, "Watchdog（VCU）: Alive 途絶 > 1000ms で「応答なし」", 12.5, "400", "start", C["sub"])
    s.text(1010, LY + 80, "　1つ → Degraded、2つ以上 → EStop（ラッチ）", 12.5, "400", "start", C["sub"])
    return s.render()


if __name__ == "__main__":
    import sys
    open(sys.argv[1], "w", encoding="utf-8").write(build())
