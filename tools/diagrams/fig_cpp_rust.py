"""Figure 3: how the C++ reference implementation and the Rust implementation relate."""
from svgkit import Svg, C, tw

W, H = 1480, 1010

ROWS = [
    ("model/*.h",
     ["純粋関数（engine・brake・steering・", "vehicledynamics・safety_logic）"],
     "model.rs", ["同じ式を f32 で移植", "（steering は厳密離散化）"], "同じ式・同じ判定"),
    ("swc/*.cpp",
     ["各 SWC。Model を呼ぶだけ"],
     "swc.rs", ["各 SWC。渡された Rte だけを読み書き"], "同じ SWC 構成"),
    ("rte/",
     ["RTE はグローバルに 1 つ", "（全 SWC で共有）"],
     "rte.rs", ["RTE は ECU ごとに 1 つ", "（信号の中身は同じ）"], "信号の中身は同じ"),
    ("bsw/diag + swc/safety_swc.cpp",
     ["生存カウンタ・Watchdog"],
     "safety.rs + model.rs", ["同じ判定: 1つ → Degraded、", "2つ以上 → EStop（ラッチ）"], "同じ判定・同じしきい値"),
    ("app/main.cpp + bsw/timebase",
     ["10/20/100ms のタスク順序", "sdv_sim --stall で故障注入"],
     "sim.rs", ["1ティックの順序（ECU 分割）", "API / シナリオで故障注入"], "遅延0・欠損0で同じ値が同じティック"),
    ("bsw/logging",
     ["CSV を出力（logs/latest.csv）"],
     "sim.rs の Row::to_csv", ["同じ列・同じ書式"], "同じ CSV"),
]


def build():
    s = Svg(W, H, "C++ と Rust の関係",
            "C++ の参照実装と Rust 実装のモジュール対応、Rust だけにあるもの、基準ログによる照合")
    s.text(W / 2, 40, "C++ と Rust の関係", 24, "700", "middle")
    s.text(W / 2, 66, "C++ は参照実装（基準ログの生成元）、Rust が主役。同じ式・同じ判定を移植し、出力が一致することを CI で確認する", 14, "400", "middle", C["sub"])

    LX, LW, RX, RW = 40, 520, 920, 520
    s.rect(LX, 84, LW, 38, *C["cpp"], 2, 8)
    s.text(LX + LW / 2, 109, "C++（参照実装）  src/", 17, "700", "middle")
    s.rect(RX, 84, RW, 38, *C["rust"], 2, 8)
    s.text(RX + RW / 2, 109, "Rust（主役）  rust/sdv-core/src/", 17, "700", "middle")

    y = 140
    for lname, llines, rname, rlines, mid in ROWS:
        h = 66
        for (x, w, name, lines, kind) in [(LX, LW, lname, llines, "cpp"), (RX, RW, rname, rlines, "rust")]:
            f, st = C[kind]
            s.rect(x, y, w, h, f, st, 1.5, 8)
            s.text(x + 14, y + 25, name, 14.5, "700", "start", st)
            s.lines(x + 14, y + 45, lines, 12.5, 16, "start", C["sub"]) if len(lines) == 1 else s.lines(x + 14, y + 42, lines, 12.5, 15, "start", C["sub"])
        # mapping arrow
        s.path(f"M{LX + LW + 6},{y + h / 2} H{RX - 6}", "#6b7280", 2, dash="6 5", marker_end="a_gray", marker_start="a_gray")
        s.label((LX + LW + RX) / 2, y + h / 2 - 8, mid, 12.5, "#374151", "middle", 5)
        y += 80

    # only-boxes
    oy = y + 4
    s.rect(LX, oy, LW, 112, "#f9fafb", "#9ca3af", 1.5, 8, dash="6 4")
    s.text(LX + 14, oy + 26, "C++ だけ", 14.5, "700", "start", C["sub"])
    s.lines(LX + 14, oy + 50, ["1 プロセス・1 つの RTE（ECU もバスもない）", "内蔵のデモシナリオ（外部化なし）", "Config / NVM なし（パラメータは定数）"], 12.5, 18, "start", C["sub"])

    s.rect(RX, oy, RW, 112, "#fffaf5", C["rust"][1], 1.5, 8, dash="6 4")
    s.text(RX + 14, oy + 26, "Rust だけ", 14.5, "700", "start", C["rust"][1])
    chips = [("bus.rs", "仮想バス"), ("failsafe.rs", "フェイルセーフ"), ("config.rs", "Config（JSON）"),
             ("nvm.rs", "疑似NVM"), ("scenario.rs", "シナリオ（JSON）"), ("lib.rs", "C ABI（WASM）")]
    for i, (fn, desc) in enumerate(chips):
        cx = RX + 14 + (i % 3) * 168
        cy = oy + 40 + (i // 3) * 33
        s.rect(cx, cy, 160, 28, "#ffffff", "#fdba74", 1, 6)
        s.text(cx + 8, cy + 18, f"{fn}  {desc}", 11.5, "400", "start", C["ink"])

    # verification band
    by = oy + 140
    s.rect(30, by, W - 60, 232, C["panel"], "#d1d5db", 1.2, 10)
    s.text(50, by + 28, "基準ログによる照合（CI で毎回確認）", 15.5, "700")

    ay = by + 70
    s.rect(60, ay, 360, 90, *C["cpp"], 2, 10)
    s.text(240, ay + 30, "C++ sdv_sim を実行", 15, "700", "middle")
    s.lines(240, ay + 54, ["通常 ・ --stall brake:2", "--stall steering:3 --stall brake:3.5", "--stall engine:1 --stall vehicledynamics:2"], 12, 15, "middle", C["sub"])

    s.rect(530, ay, 420, 90, *C["gold"], 2.2, 10)
    s.text(740, ay + 32, "基準ログ（golden）", 16, "700", "middle")
    s.lines(740, ay + 56, ["rust/sdv-core/tests/golden/*.csv", "4 本・各 10 秒・1000 行"], 12.5, 17, "middle", C["sub"])

    s.rect(1060, ay, 360, 90, *C["rust"], 2, 10)
    s.text(1240, ay + 30, "Rust が同じ走行を再現", 15, "700", "middle")
    s.lines(1240, ay + 54, ["Sim をシナリオ JSON（イベント付き）や", "API で動かし、ログ行を生成"], 12, 17, "middle", C["sub"])

    s.path(f"M420,{ay + 45} H520", "#374151", 3, marker_end="a_dark")
    s.path(f"M1050,{ay + 45} H960", "#374151", 3, marker_end="a_dark")
    s.label(470, ay + 78, "CI: golden-log", 12, "#111827", "middle", 5, "#ffffff", "700")
    s.label(1005, ay + 78, "cargo test", 12, "#111827", "middle", 5, "#ffffff", "700")
    s.text(470, ay + 118, "C++ の出力 == golden", 12.5, "400", "middle", C["sub"])
    s.text(1005, ay + 118, "Rust の出力 == golden（全行一致）", 12.5, "400", "middle", C["sub"])
    s.text(W / 2, ay + 150, "C++ の挙動を変えたら golden を更新し、Rust も合わせる（手順は CONTRIBUTING.md）。ずれると CI が落ちる", 13, "400", "middle", C["sub"])
    return s.render()


if __name__ == "__main__":
    import sys
    open(sys.argv[1], "w", encoding="utf-8").write(build())
