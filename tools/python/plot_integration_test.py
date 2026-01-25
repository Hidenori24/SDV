#!/usr/bin/env python3
"""
Engine + Brake Integration Test Visualization
統合テスト結果の波形表示ツール
"""
import csv
from pathlib import Path
import matplotlib.pyplot as plt
import matplotlib.gridspec as gridspec

def main():
    root = Path(__file__).resolve().parents[2]
    log_path = root / "build" / "logs" / "latest.csv"
    
    if not log_path.exists():
        print(f"❌ Log not found: {log_path}")
        print(f"Run simulation first: cd build && ./sdv_sim")
        return

    # データ読み込み
    t, throttle, brake, drive_accel_cmd, brake_decel_cmd = [], [], [], [], []
    v, x, y, estop, system_state = [], [], [], [], []
    
    with log_path.open() as f:
        reader = csv.DictReader(f)
        for row in reader:
            t.append(float(row["t"]))
            throttle.append(float(row["throttle"]))
            brake.append(float(row["brake"]))
            drive_accel_cmd.append(float(row["drive_accel_cmd"]))
            brake_decel_cmd.append(float(row["brake_decel_cmd"]))
            v.append(float(row["v"]))
            x.append(float(row["x"]))
            y.append(float(row["y"]))
            estop.append(int(row["estop"]))
            system_state.append(int(row["system_state"]))

    # 統計情報表示
    print("\n" + "="*60)
    print("Engine + Brake Integration Test - Simulation Results")
    print("="*60)
    print(f"シミュレーション時間: {t[-1]:.1f} s")
    print(f"サンプル数: {len(t)}")
    print(f"\n📊 統計情報:")
    print(f"  throttle       平均: {sum(throttle)/len(throttle):.3f}, 最大: {max(throttle):.3f}")
    print(f"  brake          平均: {sum(brake)/len(brake):.3f}, 最大: {max(brake):.3f}")
    print(f"  drive_accel_cmd 平均: {sum(drive_accel_cmd)/len(drive_accel_cmd):.3f} m/s², 最大: {max(drive_accel_cmd):.3f} m/s²")
    print(f"  brake_decel_cmd 平均: {sum(brake_decel_cmd)/len(brake_decel_cmd):.3f} m/s², 最大: {max(brake_decel_cmd):.3f} m/s²")
    print(f"  速度            平均: {sum(v)/len(v):.3f} m/s, 最大: {max(v):.3f} m/s")
    print(f"  移動距離        X: {x[-1]:.2f} m, Y: {y[-1]:.2f} m")
    print("="*60 + "\n")

    # プロット作成（6サブプロット）
    fig = plt.figure(figsize=(16, 10))
    gs = gridspec.GridSpec(3, 2, figure=fig, hspace=0.3, wspace=0.3)

    # 1. ドライバー入力（throttle, brake）
    ax1 = fig.add_subplot(gs[0, 0])
    ax1.plot(t, throttle, 'g-', label='throttle', linewidth=1.5)
    ax1.plot(t, brake, 'r-', label='brake', linewidth=1.5)
    ax1.set_xlabel('Time [s]')
    ax1.set_ylabel('Input [0..1]')
    ax1.set_title('🎮 Driver Input (Throttle & Brake)', fontweight='bold')
    ax1.legend(loc='upper right')
    ax1.grid(True, alpha=0.3)
    ax1.set_ylim(-0.1, 1.1)

    # 2. アクチュエータコマンド（Engine: drive_accel_cmd, Brake: brake_decel_cmd）
    ax2 = fig.add_subplot(gs[0, 1])
    ax2.plot(t, drive_accel_cmd, 'b-', label='drive_accel_cmd', linewidth=1.5)
    ax2.plot(t, brake_decel_cmd, 'orange', label='brake_decel_cmd', linewidth=1.5)
    ax2.set_xlabel('Time [s]')
    ax2.set_ylabel('Command [m/s²]')
    ax2.set_title('⚙️ Actuator Commands (Engine & Brake)', fontweight='bold')
    ax2.legend(loc='upper right')
    ax2.grid(True, alpha=0.3)
    ax2.axhline(y=0, color='gray', linestyle='--', alpha=0.5)

    # 3. 入出力相関（Throttle → Accel）
    ax3 = fig.add_subplot(gs[1, 0])
    ax3.scatter(throttle, drive_accel_cmd, alpha=0.5, s=10, c='blue')
    ax3.set_xlabel('Throttle [0..1]')
    ax3.set_ylabel('drive_accel_cmd [m/s²]')
    ax3.set_title('🔗 Engine: Throttle → Accel Correlation', fontweight='bold')
    ax3.grid(True, alpha=0.3)
    ax3.set_xlim(-0.1, 1.1)

    # 4. 入出力相関（Brake → Decel）
    ax4 = fig.add_subplot(gs[1, 1])
    ax4.scatter(brake, brake_decel_cmd, alpha=0.5, s=10, c='red')
    ax4.set_xlabel('Brake [0..1]')
    ax4.set_ylabel('brake_decel_cmd [m/s²]')
    ax4.set_title('🔗 Brake: Brake → Decel Correlation', fontweight='bold')
    ax4.grid(True, alpha=0.3)
    ax4.set_xlim(-0.1, 1.1)

    # 5. 車速
    ax5 = fig.add_subplot(gs[2, 0])
    ax5.plot(t, v, 'purple', linewidth=1.5)
    ax5.set_xlabel('Time [s]')
    ax5.set_ylabel('Velocity [m/s]')
    ax5.set_title('🚗 Vehicle Speed', fontweight='bold')
    ax5.grid(True, alpha=0.3)
    ax5.axhline(y=0, color='gray', linestyle='--', alpha=0.5)

    # 6. 軌跡（X-Y平面）
    ax6 = fig.add_subplot(gs[2, 1])
    ax6.plot(x, y, 'darkblue', linewidth=1.5)
    ax6.scatter(x[0], y[0], c='green', s=100, marker='o', label='Start', zorder=5)
    ax6.scatter(x[-1], y[-1], c='red', s=100, marker='x', label='End', zorder=5)
    ax6.set_xlabel('X [m]')
    ax6.set_ylabel('Y [m]')
    ax6.set_title('🗺️ Vehicle Trajectory (X-Y)', fontweight='bold')
    ax6.legend(loc='best')
    ax6.grid(True, alpha=0.3)
    ax6.axis('equal')

    # タイトル
    fig.suptitle('Engine + Brake SWC Integration Test Results', 
                 fontsize=16, fontweight='bold')

    # 保存
    output_path = root / "docs" / "test_reports" / "engine_brake_integration_plot.png"
    output_path.parent.mkdir(parents=True, exist_ok=True)
    plt.savefig(output_path, dpi=150, bbox_inches='tight')
    print(f"✅ Plot saved: {output_path}")

    # 表示
    plt.show()

if __name__ == "__main__":
    main()
