# W62: records the YouTube embed playing in the browser **in real time**, through QEMU's own VNC server
# (tools/media/vnc-record.py: a minimal RFB client writing to ffmpeg at a constant rate), with the
# sound the AC'97 played (QEMU's -audiodev wav) aligned by the WAV's growth, and reports the
# intervals between the pictures the guest put on screen from its own log
# (tools/media/frame-intervals.py: the truth; the recording only shows it).
#   setsid nohup python3 tools/w24-site.py 8679 > /tmp/site-pace.log 2>&1 &
#   OUT=/dev/shm/w62; VIDEO=dQw4w9WgXcQ REC=40 QEMU_MEM=256M QEMU_NIC=virtio-net-pci PORT=8679 \
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0 -vnc 127.0.0.1:7" \
#     bash tools/perf/run.sh IMG bios $OUT 800 tools/perf/scen/w62-youtube-vnc.sh
# Results: $OUT/w62.mp4 (picture and sound), $OUT/vnc.summary.txt (what the recording got),
# $OUT/intervals.txt (the guest's own measure), $OUT/rec-status.txt.
# Env: VNC_PORT (5907 = -vnc :7), FPS (output rate of the recording, default 30), REC (seconds),
# DELAY (seconds between `playing` and the recording, default 3), MOUSE=1 (the pointer moves over the
# page while it records), STATS=1 (Shift+I over the video: the statistics overlay).
# Like W60 the image needs the root of a TLS-intercepting network (development only, never committed).
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8679}
VIDEO=${VIDEO:-dQw4w9WgXcQ}
REC=${REC:-40}
FPS=${FPS:-30}
DELAY=${DELAY:-3}
VNC_PORT=${VNC_PORT:-5907}
WAITPLAY=${WAITPLAY:-400}
here=$(cd "$(dirname "$0")/../../.." && pwd)
CAP=$(printf '%s\n' "${QEMU_EXTRA:-}" | sed -n 's/.*path=\([^, ]*\).*/\1/p')
wait_for() { local i; for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done; return 1; }
# The recorder connects now and starts by itself when the player says it plays.
python3 "$here/tools/media/vnc-record.py" --port "$VNC_PORT" --out "$OUT/vnc.mkv" --fps "$FPS" \
  --secs "$REC" --serial "$OUT/serial.log" --start 'media: element [0-9]+ playing at' --delay "$DELAY" \
  --wav "${CAP:-/nonexistent}" --log "$OUT/vnc" > "$OUT/vnc.log" 2>&1 &
rec_pid=$!
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3; typestr "http://203.0.113.5:$PORT/yt?go=1&v=$VIDEO&p=autoplay=1"; key ret
if wait_for "media: element [0-9]* playing at" "$WAITPLAY"; then
  echo "playing" > "$OUT/rec-status.txt"
  sleep "$DELAY"
  if [ "${STATS:-0}" = 1 ]; then
    # The page's box (the player fills it): Shift+I with the pointer over the video.
    geo=$(grep -a "handed to pid" "$OUT/serial.log" | tail -1 | sed -n 's/.*page at \([0-9]*\),\([0-9]*\) size \([0-9]*\)x\([0-9]*\).*/\1 \2 \3 \4/p')
    set -- $geo
    goto $(( ${1:-0} + ${3:-1280} / 2 )) $(( ${2:-60} + ${4:-600} / 3 ))
    key shift-i
  fi
  t0=$(date +%s)
  n=0
  while [ $(( $(date +%s) - t0 )) -lt "$REC" ]; do
    if [ "${MOUSE:-0}" = 1 ]; then
      if [ $((n % 2)) = 0 ]; then move 40 0; else move -40 0; fi
    fi
    n=$((n + 1)); sleep 0.2
  done
else
  echo "never played" > "$OUT/rec-status.txt"
fi
# The recorder ends by itself; give it the time to close the file.
for ((k = 0; k < 60; k++)); do kill -0 "$rec_pid" 2>/dev/null || break; sleep 1; done
kill "$rec_pid" 2>/dev/null
sleep 1
if [ -n "$CAP" ]; then
  # QEMU writes the WAV's header when it quits.
  mon quit
  for ((k = 0; k < 40; k++)); do
    echo "info status" | socat - "UNIX-CONNECT:${MON_SOCK:-$(cat "$OUT/mon.path")}" >/dev/null 2>&1 || break
    sleep 0.5
  done
  sleep 1
  if [ -s "$OUT/vnc.mkv" ]; then
    python3 "$here/tools/media/vnc-mux.py" "$OUT/vnc.mkv" "$CAP" "$OUT/w62.mp4" --log "$OUT/vnc" \
      > "$OUT/mux.log" 2>&1
  fi
fi
python3 "$here/tools/media/frame-intervals.py" "$OUT/serial.log" > "$OUT/intervals.txt" 2>&1
cat "$OUT/vnc.summary.txt" >> "$OUT/rec-status.txt" 2>/dev/null
finish
