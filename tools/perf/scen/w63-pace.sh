# W63: frame pacing of the browser's video player without YouTube: a generated clip (testsrc2 and a
# tone, VP9 + Opus, FPS frames a second, SIZE pixels) in an 853x480 box, from tools/w24-site.py's
# /pace page, played for REC seconds; the guest logs when each picture reached the screen and
# tools/media/frame-intervals.py turns that into the intervals (mean, standard deviation, percentiles,
# hitches). The same load every run (no page scripts, no network), so pacing changes can be compared.
#   setsid nohup python3 tools/w24-site.py 8679 > /tmp/site-pace.log 2>&1 &
#   OUT=/dev/shm/w63; FPS=25 PORT=8679 QEMU_MEM=256M \
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
#     bash tools/perf/run.sh IMG bios $OUT 400 tools/perf/scen/w63-pace.sh
# Result: $OUT/intervals.txt. Env: FPS (25), SIZE (426x240), SECS (length of the clip, 40), REC
# (seconds measured, 30), MOUSE=1 (the pointer moves over the page meanwhile), STATS=1 (Shift+I: the
# statistics overlay on, so its cost is in the measure).
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8679}
FPS=${FPS:-25}
SIZE=${SIZE:-426x240}
SECS=${SECS:-40}
REC=${REC:-30}
here=$(cd "$(dirname "$0")/../../.." && pwd)
wait_for() { local i; for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done; return 1; }
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3; typestr "http://203.0.113.5:$PORT/pace?fps=$FPS&size=$SIZE&secs=$SECS"; key ret
if wait_for "media: element [0-9]* playing at" 240; then
  echo "playing" > "$OUT/rec-status.txt"
  if [ "${STATS:-0}" = 1 ]; then
    # Over the video (the box starts at the page's top left, under the browser's bar).
    goto 420 300
    key shift-i
  fi
  t0=$(date +%s); n=0
  while [ $(( $(date +%s) - t0 )) -lt "$REC" ]; do
    if [ "${MOUSE:-0}" = 1 ]; then
      if [ $((n % 2)) = 0 ]; then move 40 0; else move -40 0; fi
    fi
    n=$((n + 1)); sleep 0.2
  done
  shot pace
else
  echo "never played" > "$OUT/rec-status.txt"
fi
python3 "$here/tools/media/frame-intervals.py" "$OUT/serial.log" --skip 3 > "$OUT/intervals.txt" 2>&1
finish
