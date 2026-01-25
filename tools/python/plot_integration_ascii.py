#!/usr/bin/env python3
"""
Engine + Brake Integration Test - ASCII Visualization
統合テスト結果の簡易波形表示（matplotlib不要）
"""
import csv
from pathlib import Path

def plot_ascii(title, data, time, width=80, height=15):
    """ASCII art でプロット"""
    print(f"\n{'='*width}")
    print(f"{title:^{width}}")
    print('='*width)
    
    if not data:
        print("(No data)")
        return
    
    min_val, max_val = min(data), max(data)
    range_val = max_val - min_val if max_val != min_val else 1
    
    # 統計
    avg = sum(data) / len(data)
    print(f"Min: {min_val:.3f}, Max: {max_val:.3f}, Avg: {avg:.3f}, Samples: {len(data)}")
    print()
    
    # Y軸のラベル
    for i in range(height):
        y = max_val - (i / (height - 1)) * range_val
        row = f"{y:6.2f} │"
        
        for j in range(len(data)):
            val = data[j]
            normalized = (val - min_val) / range_val if range_val else 0
            row_level = 1 - (i / (height - 1))
            
            if abs(normalized - row_level) < 0.5 / height:
                row += "█"
            elif j < len(data) - 1:
                next_val = data[j + 1]
                next_normalized = (next_val - min_val) / range_val if range_val else 0
                if min(normalized, next_normalized) <= row_level <= max(normalized, next_normalized):
                    row += "│"
                else:
                    row += " "
            else:
                row += " "
        
        print(row)
    
    # X軸
    print(f"{'       └' + '─' * len(data)}")
    print(f"       0{' ' * (len(data)//2 - 1)}{time[-1]:.1f}s")

def main():
    root = Path(__file__).resolve().parents[2]
    log_path = root / "build" / "logs" / "latest.csv"
    
    if not log_path.exists():
        print(f"❌ Log not found: {log_path}")
        print(f"Run simulation first: cd build && ./sdv_sim")
        return

    # データ読み込み
    t, throttle, brake, drive_accel_cmd, brake_decel_cmd = [], [], [], [], []
    v = []
    
    with log_path.open() as f:
        reader = csv.DictReader(f)
        for row in reader:
            t.append(float(row["t"]))
            throttle.append(float(row["throttle"]))
            brake.append(float(row["brake"]))
            drive_accel_cmd.append(float(row["drive_accel_cmd"]))
            brake_decel_cmd.append(float(row["brake_decel_cmd"]))
            v.append(float(row["v"]))
    
    # サンプリング（表示用に間引く）
    step = max(1, len(t) // 80)
    t_sampled = t[::step]
    throttle_sampled = throttle[::step]
    brake_sampled = brake[::step]
    accel_sampled = drive_accel_cmd[::step]
    decel_sampled = brake_decel_cmd[::step]
    v_sampled = v[::step]
    
    print("\n" + "="*80)
    print("Engine + Brake Integration Test - Simulation Results".center(80))
    print("="*80)
    print(f"\n📊 データ概要:")
    print(f"  シミュレーション時間: {t[-1]:.1f} s")
    print(f"  サンプル数: {len(t)} (表示: {len(t_sampled)})")
    
    # プロット
    plot_ascii("🎮 Driver Input: Throttle", throttle_sampled, t_sampled)
    plot_ascii("🎮 Driver Input: Brake", brake_sampled, t_sampled)
    plot_ascii("⚙️ Engine Command: drive_accel_cmd [m/s²]", accel_sampled, t_sampled)
    plot_ascii("⚙️ Brake Command: brake_decel_cmd [m/s²]", decel_sampled, t_sampled)
    plot_ascii("🚗 Vehicle Speed [m/s]", v_sampled, t_sampled)
    
    # 相関分析（テキスト）
    print(f"\n{'='*80}")
    print("🔗 Input-Output Correlation Analysis".center(80))
    print('='*80)
    
    # Throttle → Accel
    non_zero_throttle = [(throttle[i], drive_accel_cmd[i]) 
                         for i in range(len(throttle)) if throttle[i] > 0.01]
    if non_zero_throttle:
        avg_ratio = sum(acc/thr for thr, acc in non_zero_throttle) / len(non_zero_throttle)
        print(f"\n✅ Engine: throttle → drive_accel_cmd")
        print(f"   平均スケール係数: {avg_ratio:.2f} (期待値: 2.0 m/s²)")
    
    # Brake → Decel
    non_zero_brake = [(brake[i], brake_decel_cmd[i]) 
                      for i in range(len(brake)) if brake[i] > 0.01]
    if non_zero_brake:
        avg_ratio = sum(dec/brk for brk, dec in non_zero_brake) / len(non_zero_brake)
        print(f"\n✅ Brake: brake → brake_decel_cmd")
        print(f"   平均スケール係数: {avg_ratio:.2f} (期待値: 4.0 m/s²)")
    
    print(f"\n{'='*80}")
    print("✅ ASCII波形表示完了".center(80))
    print('='*80)
    print(f"\n💡 詳細なグラフを見るには:")
    print(f"   1. pip3 install matplotlib")
    print(f"   2. python3 tools/python/plot_integration_test.py")
    print()

if __name__ == "__main__":
    main()
