#!/usr/bin/env bash
# gen-test-media.sh OUTDIR: WebM test files (VP9 + Opus, like YouTube's itags 243/244/247 + 251) and
# their references, made with the host's ffmpeg (libvpx-vp9 and libopus).
#
# For each NAME (v180, v360, v720: 320x180, 640x360, 1280x720, 3 s, 30 fps, a keyframe per second):
#   NAME.webm        the file (testsrc2 pattern; stereo tones, 440 Hz left, 660 + 3000 Hz right)
#   NAME.vmd5        `ffmpeg -c:v libvpx-vp9 -f framemd5`: the MD5 of every decoded frame (I420)
#   NAME.ref.raw     the audio decoded by ffmpeg (s16le, stereo, 48 kHz), the SNR reference
#   NAME.libopus.raw the same decoded by libopus (float build) through ffmpeg
#   NAME.rgba        the first picture converted to RGBA by ffmpeg (swscale), to check the colours
# Also `small.webm` (320x180, 1 s, low rates, mono audio), the fixture of kitsune_core's webm
# tests, and its packet list `small.packets.txt` (stream, pts in ms, keyframe flag, size).
# Used by tools/media/check-host.sh and tools/perf/scen/w57-mediadec.sh. See docs/design/midia.md.
set -eu
out=$1
mkdir -p "$out"
gen() { # gen SIZE VIDEO_RATE SECS NAME
  ffmpeg -v error -y -f lavfi -i "testsrc2=size=$1:rate=30" \
    -f lavfi -i "aevalsrc=0.5*sin(440*2*PI*t)|0.4*sin(660*2*PI*t)+0.1*sin(3000*2*PI*t):s=48000" \
    -t "$3" -c:v libvpx-vp9 -b:v "$2" -g 30 -c:a libopus -b:a 96k "$out/$4.webm"
  ffmpeg -v error -y -c:v libvpx-vp9 -i "$out/$4.webm" -map 0:v -f framemd5 "$out/$4.vmd5"
  ffmpeg -v error -y -i "$out/$4.webm" -map 0:a -f s16le -ac 2 -ar 48000 "$out/$4.ref.raw"
  ffmpeg -v error -y -c:a libopus -i "$out/$4.webm" -map 0:a -f s16le -ac 2 -ar 48000 "$out/$4.libopus.raw"
  ffmpeg -v error -y -c:v libvpx-vp9 -i "$out/$4.webm" -map 0:v -frames:v 1 -sws_flags +accurate_rnd+bitexact \
    -pix_fmt rgba -f rawvideo "$out/$4.rgba"
}
gen 320x180 100k 3 v180
gen 640x360 300k 3 v360
gen 1280x720 1000k 3 v720
ffmpeg -v error -y -f lavfi -i "testsrc2=size=320x180:rate=30" -f lavfi -i "sine=frequency=440:sample_rate=48000" \
  -t 1 -c:v libvpx-vp9 -b:v 60k -g 15 -c:a libopus -b:a 24k "$out/small.webm"
ffprobe -v error -show_entries packet=stream_index,pts,flags,size -of csv=p=0 "$out/small.webm" \
  | awk -F, '{ k = ($4 ~ /K/) ? 1 : 0; print $1, $2, k, $3 }' > "$out/small.packets.txt"
ls -la "$out"
