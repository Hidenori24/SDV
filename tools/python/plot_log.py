#!/usr/bin/env python3
import csv
from pathlib import Path
import matplotlib.pyplot as plt

def main():
    root = Path(__file__).resolve().parents[2]
    log_path = root / "logs" / "latest.csv"
    if not log_path.exists():
        raise SystemExit(f"Log not found: {log_path}")

    t, v, omega, yaw = [], [], [], []
    with log_path.open() as f:
        reader = csv.DictReader(f)
        for row in reader:
            t.append(float(row["t"]))
            v.append(float(row["v"]))
            omega.append(float(row["wheel_omega"]))
            yaw.append(float(row["yaw"]))

    plt.figure()
    plt.plot(t, v)
    plt.xlabel("t [s]")
    plt.ylabel("v [m/s]")
    plt.title("Vehicle Speed")
    plt.show()

    plt.figure()
    plt.plot(t, omega)
    plt.xlabel("t [s]")
    plt.ylabel("wheel_omega [rad/s]")
    plt.title("Wheel Angular Velocity")
    plt.show()

    plt.figure()
    plt.plot(t, yaw)
    plt.xlabel("t [s]")
    plt.ylabel("yaw [rad]")
    plt.title("Yaw")
    plt.show()

if __name__ == "__main__":
    main()
