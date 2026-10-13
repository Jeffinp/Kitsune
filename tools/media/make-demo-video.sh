#!/usr/bin/env bash
# make-demo-video.sh OUT_DIR AUDIO.wav OUT.mp4 [AUDIO_OFFSET_S]
# Builds an MP4 from the frames of tools/perf/scen/w61-youtube-record.sh (rec-NNNN.ppm and
# rec-times.txt in OUT_DIR) and the WAV that QEMU recorded. The frame rate is the measured one
# (frames over seconds), so the picture plays in real time; the sound starts AUDIO_OFFSET_S (default 0)
# into the WAV.
set -euo pipefail
dir=$1; wav=$2; out=$3; off=${4:-0}
n=$(ls "$dir"/rec-*.ppm | wc -l)
first=$(head -1 "$dir/rec-times.txt"); last=$(tail -1 "$dir/rec-times.txt")
fps=$(python3 -c "print(round(($n-1)/($last-$first), 3))")
dur=$(python3 -c "print(round($last-$first, 2))")
echo "$n frames in $dur s = $fps fps"
ffmpeg -loglevel error -y -framerate "$fps" -i "$dir/rec-%04d.ppm" -ss "$off" -i "$wav" -t "$dur" \
  -c:v libx264 -pix_fmt yuv420p -crf 26 -c:a aac -b:a 128k -shortest "$out"
ls -la "$out"
