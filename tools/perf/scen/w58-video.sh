# W58: the media player of the browser: <video> with a WebM file, the same media through Media
# Source Extensions, sound into the mixer, the picture on screen, seek, end, the controls.
#
#   tools/w24-site.py 8179 &          (the pages /video and /mse, the files made with ffmpeg)
#   OUT=/tmp/w58; QEMU_MEM=256M PORT=8179 \
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
#     bash tools/perf/run.sh IMG bios $OUT 600 tools/perf/scen/w58-video.sh
#
# /video: `<video controls src=/media/v360.webm>` (3 s, 640x360 VP9, a 440 Hz Opus tone). Its script
# plays it once it can, and after the end seeks to 1 s and plays to the end again. Screenshots while
# it plays must differ (the picture moves and the time on the bar runs); the page's events are in the
# serial log (`media-test: ...`). Then a click on the controls' play button plays it again (from the
# start). /mse: the same media from two WebM DASH files (video and sound apart), the init segments and
# the clusters fetched with Range requests and appended to two SourceBuffers; played, seeked, ended.
# The verdict ($OUT/verdict.txt) checks the events, the pictures, and, on the WAV QEMU recorded, the
# sound: 440 Hz for 3 s, then 2 s (after the seek), for each page (tools/perf/audio-check.py). Without
# a sound device (no QEMU_EXTRA) the sound part is skipped and the system clock must have run.
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8079}
wait_for() { # wait_for PATTERN SECONDS
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
count() { grep -ac -- "$1" "$OUT/serial.log" 2>/dev/null || true; }
wait_n() { # wait_n PATTERN N SECONDS
  local i
  for ((i = 0; i < $3 * 2; i++)); do [ "$(count "$1")" -ge "$2" ] && return 0; sleep 0.5; done
  return 1
}
open_url() { key ctrl-l; sleep 0.3; typestr "$1"; key ret; }

wait_first_frame
dock_icon browser; click; sleep 3
open_url "http://203.0.113.5:$PORT/video"
wait_for "media-test: canPlayType" 120
wait_for "media-test: playing" 120
# Two pictures while it plays, ~1 s apart.
sleep 0.4; snap v1-playing
sleep 1.0; snap v2-playing
wait_for "media-test: first end" 60
sleep 0.3; snap v3-ended
wait_for "media-test: DONE" 60
sleep 1
shot v4-done
# The controls: the play button at the left of the bar (VIDEO_PLAY_X/Y: the screen point).
goto "${VIDEO_PLAY_X:-187}" "${VIDEO_PLAY_Y:-563}"; click
wait_n "media-test: play " 3 30
sleep 1.2; snap v5-controls
wait_n "media-test: ended" 3 30
sleep 1

open_url "http://203.0.113.5:$PORT/mse"
wait_for "media-test: endOfStream" 120
wait_n "media-test: playing" 4 60
sleep 0.4; snap m1-playing
sleep 1.0; snap m2-playing
wait_n "media-test: DONE" 2 90
sleep 1
shot m3-done
flush_snaps

# QEMU completes the WAV (its header) only when it quits.
CAP=$(printf '%s\n' "${QEMU_EXTRA:-}" | sed -n 's/.*path=\([^, ]*\).*/\1/p')
if [ -n "$CAP" ]; then
  mon quit
  for ((i = 0; i < 40; i++)); do
    echo "info status" | socat - "UNIX-CONNECT:${MON_SOCK:-$(cat "$OUT/mon.path")}" >/dev/null 2>&1 || break
    sleep 0.5
  done
  sleep 1
fi

{
  grep -a "\[jsrun:" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' > "$OUT/js-out.txt"
  ok=1
  need() { if grep -aq -- "$1" "$OUT/js-out.txt"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needlog() { if grep -aq -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needn() { local n; n=$(grep -ac -- "$1" "$OUT/js-out.txt"); if [ "$n" -ge "$2" ]; then echo "PASS $3 ($n)"; else echo "MISS $3 ($n of $2)"; ok=0; fi; }
  needlog "browser: player for element" "a player was made for the element"
  needlog "proc: started 'mediadec'" "a decoder process started"
  need "^media-test: canPlayType probably" "canPlayType webm vp9+opus = probably"
  need "^media-test: loadedmetadata t=0.000 rs=1 d=3" "loadedmetadata with the duration"
  need "^media-test: size 640x360" "videoWidth/videoHeight"
  need "^media-test: play resolved" "play() resolved"
  needn "^media-test: playing" 2 "playing (start and after the seek)"
  needn "^media-test: timeupdate" 2 "timeupdate while playing"
  need "^media-test: first end at 3" "ended at 3 s"
  need "^media-test: seeked t=1.0" "seeked to 1 s"
  need "^media-test: DONE played" "ended again after the seek"
  needn "^media-test: play " 3 "the controls' play button played it again"
  needlog "media: element [0-9]* played " "playback stretches in the log"
  # The pictures change while it plays.
  for pair in "v1-playing v2-playing" "m1-playing m2-playing"; do
    set -- $pair
    if [ -f "$OUT/$1.png" ] && [ -f "$OUT/$2.png" ]; then
      d=$(compare -metric AE "$OUT/$1.png" "$OUT/$2.png" null: 2>&1 | cut -d' ' -f1)
      if [ "${d%.*}" -gt 2000 ] 2>/dev/null; then echo "PASS the picture moves ($1 / $2: $d pixels differ)"; else echo "MISS the picture does not move ($1 / $2: $d)"; ok=0; fi
    else
      echo "MISS screenshots $1 $2"; ok=0
    fi
  done
  # MSE.
  need "^media-test: MediaSource function true" "MediaSource and isTypeSupported"
  need "^media-test: sourceopen open" "sourceopen"
  need "^media-test: segment 2 video 3" "segments appended (the buffers grow)"
  need "^media-test: endOfStream ended duration 3" "endOfStream"
  need "^media-test: sourceended" "sourceended"
  needn "^media-test: DONE played" 2 "MSE: played to the end, seeked, ended again"
  grep -a "media: element\|browser: player\|audio: " "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\] //' | head -40
  if [ -n "$CAP" ] && [ -f "$CAP" ]; then
    needlog "audio: AC'97 up" "AC'97 driver up"
    # /video: 3 s, then 2 s after the seek, then 3 s from the controls; /mse: 3 s, then 2 s.
    python3 -I "$(dirname "$0")/../audio-check.py" "$CAP" "440:3000;440:2000;440:3000;440:3000;440:2000" || ok=0
  else
    needlog "clock system" "without sound the system clock ran"
  fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
