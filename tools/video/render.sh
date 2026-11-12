#!/usr/bin/env bash
# 動画を最初から作り直す。
#   tools/video/render.sh            … 2 本とも
#   tools/video/render.sh intro      … 紹介動画（docs/media/sdv-intro.mp4）
#   tools/video/render.sh sim-real   … シミュレータと実機（docs/media/sdv-sim-vs-real.mp4）
# 必要: Node 20+, Python 3（numpy, scipy）, ffmpeg, Chrome/Chromium（playwright が使えるもの）
set -euo pipefail
cd "$(dirname "$0")"

# シーン境界は各 HTML の SC（時間割）と合わせる
scenes_of() {
  case "$1" in
    intro)    echo "0 7 17 28 39 49 61 76 82" ;;
    sim-real) echo "0 7 19 31 45 57 71 80 85" ;;
    *) echo "unknown movie: $1" >&2; exit 2 ;;
  esac
}
output_of() {
  case "$1" in
    intro)    echo "sdv-intro" ;;
    sim-real) echo "sdv-sim-vs-real" ;;
  esac
}

[ -d node_modules ] || npm install
python3 gen_golden.py

python3 -m http.server 8123 >/dev/null 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
sleep 1

MOVIES=("$@")
[ ${#MOVIES[@]} -gt 0 ] || MOVIES=(intro sim-real)
mkdir -p out ../../docs/media
for movie in "${MOVIES[@]}"; do
  read -r -a T <<< "$(scenes_of "$movie")"
  name="$(output_of "$movie")"
  python3 audio.py "$movie"
  : > "out/${movie}_list.txt"
  for ((i = 0; i < ${#T[@]} - 1; i++)); do
    sc="${movie//-/}_s$((i + 1))"
    # 直したシーンだけ撮り直すには、out/<シーン名>.mp4 を消してから実行する
    [ -f "out/$sc.mp4" ] || node capture.mjs "$movie" scene "$sc" "${T[i]}" "${T[i + 1]}"
    echo "file '$sc.mp4'" >> "out/${movie}_list.txt"
  done
  ffmpeg -y -loglevel error -f concat -safe 0 -i "out/${movie}_list.txt" -c copy "out/${movie}_joined.mp4"
  ffmpeg -y -loglevel error -i "out/${movie}_joined.mp4" -i "audio_${movie}.wav" -map 0:v -map 1:a \
    -c:v copy -c:a aac -b:a 192k -movflags +faststart "out/${name}-1080p.mp4"
  # リポジトリに置くのは軽い 720p 版
  ffmpeg -y -loglevel error -i "out/${name}-1080p.mp4" -vf scale=1280:720:flags=lanczos \
    -c:v libx264 -crf 25 -preset slow -pix_fmt yuv420p -c:a aac -b:a 128k -movflags +faststart "../../docs/media/${name}.mp4"
  echo "done: out/${name}-1080p.mp4, docs/media/${name}.mp4"
done
