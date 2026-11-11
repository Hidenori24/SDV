#!/usr/bin/env python3
"""基準ログ（rust/sdv-core/tests/golden/*.csv）から、動画が読む golden.js を作る。"""
import csv
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
GOLDEN = HERE.parents[1] / "rust" / "sdv-core" / "tests" / "golden"
SOURCES = {"demo": "cpp_latest.csv", "stall": "cpp_stall_steering3_brake3.5.csv"}


def rows(name):
    with open(GOLDEN / name, newline="") as f:
        data = list(csv.DictReader(f))
    # 20 ms ごと（1 行おき）に間引く。index.html の rowAt() がこの間隔を前提にしている
    return [
        [float(r["t"]), float(r["x"]), float(r["y"]), float(r["yaw"]), float(r["v"]),
         float(r["throttle"]), float(r["brake"]), float(r["steer"]),
         int(r["system_state"]), int(r["estop"])]
        for r in data[::2]
    ]


def main():
    out = {key: rows(name) for key, name in SOURCES.items()}
    (HERE / "golden.js").write_text("window.GOLDEN=" + json.dumps(out), encoding="utf-8")
    print("wrote", HERE / "golden.js")


if __name__ == "__main__":
    main()
