#!/usr/bin/env python3
"""動画の BGM（48 kHz ステレオ）を合成して audio_<動画名>.wav に書く。numpy と scipy が必要。

    python3 audio.py intro      # 紹介動画（82 秒）
    python3 audio.py sim-real   # シミュレータと実機（85 秒）

シーンの開始時刻は、各 HTML の SC（時間割）と合わせる。
"""
import sys
import wave

import numpy as np
from scipy.signal import lfilter

MOVIES = {
    # scenes: シーンの開始時刻。quiet: 伴奏を止める区間。beeps: 警告音 (開始, 終了)。chime: 完了音の時刻
    "intro": dict(duration=82.0, scenes=[0, 7, 17, 28, 39, 49, 61, 76], quiet=(33.6, 39.0),
                  blips=[31.64, 31.82], beeps=(33.64, 37.6), chime=None, bright_from=61),
    "sim-real": dict(duration=85.0, scenes=[0, 7, 19, 31, 45, 57, 71, 80], quiet=None,
                     blips=[], beeps=None, chime=68.0, bright_from=71),
}
# シーンごとの和音（MIDI ノート）と、その上で鳴らす分散和音
CHORDS = [[38, 50, 57, 62, 65], [38, 53, 57, 60, 65], [34, 53, 58, 62, 65], [33, 52, 57, 60, 64],
          [41, 53, 57, 60, 67], [36, 52, 55, 60, 67], [41, 53, 60, 65, 69, 72], [38, 50, 57, 62, 66, 69]]
ARPS = [None, [62, 65, 69, 72], [62, 65, 70, 74], [60, 64, 69, 72], [60, 65, 69, 72], [60, 64, 67, 72], [65, 69, 72, 77], None]

SR = 48000
BPM = 96


def note(m):
    return 440 * 2 ** ((m - 69) / 12)


def lowpass(x, fc):
    a = np.exp(-2 * np.pi * fc / SR)
    return lfilter([1 - a], [1, -a], x)


def synth(name):
    cfg = MOVIES[name]
    dur, scenes = cfg["duration"], cfg["scenes"]
    n = int(SR * dur)
    t = np.arange(n) / SR
    rng = np.random.default_rng(3)
    left, right = np.zeros(n), np.zeros(n)

    def add(i0, w, pan=0.5):
        w = w[: max(0, n - i0)]
        left[i0:i0 + len(w)] += w * (1 - pan) * 2 if pan != 0.5 else w
        right[i0:i0 + len(w)] += w * pan * 2 if pan != 0.5 else w

    quiet = cfg["quiet"]
    is_quiet = (lambda x: quiet is not None and quiet[0] < x < quiet[1])

    # パッド（シーンごとに和音を替える）
    for k, start in enumerate(scenes):
        end = scenes[k + 1] if k + 1 < len(scenes) else dur
        g = np.clip((t - start + 0.6) / 1.6, 0, 1) * np.clip((end + 1.0 - t) / 1.6, 0, 1)
        g = g * g * (3 - 2 * g)
        for j, m in enumerate(CHORDS[k % len(CHORDS)]):
            f = note(m)
            for det, pan in ((-0.004, 0.25), (0.004, 0.75)):
                ph = 2 * np.pi * f * (1 + det) * t + rng.uniform(0, 6.28)
                w = np.sin(ph) + 0.35 * np.sin(2 * ph) + 0.18 * np.sin(3 * ph)
                w *= g * (0.5 + 0.5 * np.sin(2 * np.pi * (0.07 + 0.013 * j) * t + j)) * 0.05 * (1.6 if j == 0 else 1)
                left[:] += w * (1 - pan)
                right[:] += w * pan

    # 分散和音（2 番目のシーンから、最後のシーンの手前まで）
    step = 60 / BPM / 2
    tt, i = float(scenes[1]), 0
    while tt < scenes[-1] - 0.5:
        k = max(j for j, s in enumerate(scenes) if s <= tt)
        arp = ARPS[k % len(ARPS)] or ARPS[1]
        m = arp[(i * 3) % 4] + (12 if i % 8 == 5 else 0)
        amp = 0.07 * (0.5 + 0.5 * min(1, (tt - scenes[1]) / 6))
        if is_quiet(tt):
            amp = 0.0
        if tt >= cfg["bright_from"]:
            amp *= 1.25
        x = np.arange(int(0.5 * SR)) / SR
        w = (np.sin(2 * np.pi * note(m) * x) + 0.5 * np.sin(4 * np.pi * note(m) * x) * np.exp(-x * 14)) * np.exp(-x * 9) * amp
        pan = 0.3 + 0.4 * ((i % 4) / 3)
        i0 = int(tt * SR)
        left[i0:i0 + len(w[: n - i0])] += w[: n - i0] * (1 - pan)
        right[i0:i0 + len(w[: n - i0])] += w[: n - i0] * pan
        j0 = i0 + int(step * 1.5 * SR)   # やまびこ
        if j0 + len(w) < n:
            left[j0:j0 + len(w)] += w * 0.35 * pan
            right[j0:j0 + len(w)] += w * 0.35 * (1 - pan)
        tt += step
        i += 1

    # キック（3 番目のシーンから）
    tt = float(scenes[2])
    while tt < scenes[-1] - 0.5:
        if not is_quiet(tt - 0.5) and not is_quiet(tt):
            x = np.arange(int(0.35 * SR)) / SR
            add(int(tt * SR), np.sin(2 * np.pi * (45 * x + (110 - 45) / 30 * (1 - np.exp(-x * 30)))) * np.exp(-x * 11) * 0.22)
        tt += 60 / BPM

    # シーンの頭の低音と、その手前のノイズの立ち上がり
    for s in scenes:
        s0 = max(s, 0.3)
        x = np.arange(int(2.5 * SR)) / SR
        add(int(s0 * SR), np.sin(2 * np.pi * (38 * x + 25 * (1 - np.exp(-x * 8)) / 8)) * np.exp(-x * 1.8) * 0.32)
        if s > 1:
            ln = int(1.6 * SR)
            add(int((s - 1.6) * SR), lowpass(rng.standard_normal(ln), 2500) * (np.arange(ln) / ln) ** 3 * 0.25)

    def beep(t0, f, d, a):
        x = np.arange(int(d * SR)) / SR
        w = np.sign(np.sin(2 * np.pi * f * x)) * 0.4 + np.sin(2 * np.pi * f * x) * 0.6
        add(int(t0 * SR), lowpass(w, 3000) * np.minimum(1, x * 200) * np.minimum(1, (d - x) * 200) * a)

    for b in cfg["blips"]:          # Degraded
        beep(b, 660, 0.12, 0.10)
    if cfg["beeps"]:                # EStop
        tb = cfg["beeps"][0]
        while tb < cfg["beeps"][1]:
            beep(tb, 880, 0.16, 0.12)
            beep(tb + 0.22, 660, 0.16, 0.10)
            tb += 0.66
    if cfg["chime"] is not None:    # commit
        for k, m in enumerate([77, 81, 84]):
            x = np.arange(int(1.2 * SR)) / SR
            add(int((cfg["chime"] + 0.12 * k) * SR), np.sin(2 * np.pi * note(m) * x) * np.exp(-x * 4) * 0.10)

    mix = np.stack([left, right], 1)
    mix *= (np.clip(t / 1.0, 0, 1) * np.clip((dur - t) / 2.2, 0, 1))[:, None]
    mix = np.tanh(mix * 1.5)
    mix = mix / np.max(np.abs(mix)) * 0.89
    out = f"audio_{name}.wav"
    with wave.open(out, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes((mix * 32767).astype("<i2").tobytes())
    print("wrote", out, f"{dur:.0f} s")


if __name__ == "__main__":
    names = sys.argv[1:] or list(MOVIES)
    for nm in names:
        if nm not in MOVIES:
            sys.exit(f"unknown movie: {nm}（{' / '.join(MOVIES)}）")
        synth(nm)
