#!/usr/bin/env bash
# 紹介動画を最初から作り直す（docs/media/sdv-intro.mp4 を上書き）。
# 必要: Node 20+, Python 3（numpy, scipy）, ffmpeg, Chrome/Chromium（playwright が使えるもの）
set -euo pipefail
cd "$(dirname "$0")"

[ -d node_modules ] || npm install
python3 gen_golden.py
python3 audio.py

python3 -m http.server 8123 >/dev/null 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
sleep 1

# シーン境界は index.html の SC（タイムライン）と合わせる
SCENES=("s1 0 7" "s2 7 17" "s3 17 28" "s4 28 39" "s5 39 49" "s6 49 61" "s7 61 76" "s8 76 82")
: > list.txt
for sc in "${SCENES[@]}"; do
  set -- $sc
  [ -f "out/$1.mp4" ] || node capture.mjs scene "$1" "$2" "$3"   # 作り直すシーンは out/<名前>.mp4 を消す
  echo "file 'out/$1.mp4'" >> list.txt
done

ffmpeg -y -loglevel error -f concat -safe 0 -i list.txt -c copy out/joined.mp4
ffmpeg -y -loglevel error -i out/joined.mp4 -i audio.wav -map 0:v -map 1:a -c:v copy -c:a aac -b:a 192k -movflags +faststart out/sdv-intro-1080p.mp4
# リポジトリに置くのは軽い 720p 版
ffmpeg -y -loglevel error -i out/sdv-intro-1080p.mp4 -vf scale=1280:720:flags=lanczos -c:v libx264 -crf 25 -preset slow -pix_fmt yuv420p -c:a aac -b:a 128k -movflags +faststart ../../docs/media/sdv-intro.mp4
echo "done: out/sdv-intro-1080p.mp4, docs/media/sdv-intro.mp4"
