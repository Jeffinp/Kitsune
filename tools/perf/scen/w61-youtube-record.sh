# W61: records the YouTube embed playing in the browser: screenshots as fast as the monitor gives them
# for REC seconds once the player says it is playing, and the sound the AC'97 driver played. The frames
# are $OUT/rec-NNNN.ppm and their times are in $OUT/rec-times.txt; tools/media/make-demo-video.sh puts
# them and the WAV into an MP4.
#   setsid nohup python3 tools/w24-site.py 8479 > /tmp/site-yt.log 2>&1 &
#   OUT=/tmp/w61; VIDEO=dQw4w9WgXcQ REC=40 QEMU_MEM=256M QEMU_NIC=virtio-net-pci PORT=8479 \
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
#     bash tools/perf/run.sh IMG bios $OUT 600 tools/perf/scen/w61-youtube-record.sh
# Like W60 the image needs the root of a TLS-intercepting network (development only, never committed).
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8479}
VIDEO=${VIDEO:-dQw4w9WgXcQ}
REC=${REC:-40}
WAITPLAY=${WAITPLAY:-300}
wait_for() { local i; for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done; return 1; }
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3; typestr "http://203.0.113.5:$PORT/yt?go=1&v=$VIDEO&p=autoplay=1"; key ret
if wait_for "media: element [0-9]* playing at" "$WAITPLAY"; then
  echo "playing" > "$OUT/rec-status.txt"
  sleep 3
  : > "$OUT/rec-times.txt"
  t0=$(date +%s.%N); n=0
  while [ "$(echo "$(date +%s.%N) - $t0 < $REC" | bc)" = 1 ]; do
    mon "screendump $OUT/rec-$(printf %04d $n).ppm"
    date +%s.%N >> "$OUT/rec-times.txt"
    # MOUSE=1: the pointer moves while the video plays (the [trace] input lines of a perf-trace
    # build then say how long the screen took to follow it).
    if [ "${MOUSE:-0}" = 1 ]; then
      if [ $((n % 2)) = 0 ]; then move 40 0; else move -40 0; fi
    fi
    n=$((n + 1)); sleep 0.2
  done
  echo "frames $n" >> "$OUT/rec-status.txt"
else
  echo "never played" > "$OUT/rec-status.txt"
fi
sleep 2
CAP=$(printf '%s\n' "${QEMU_EXTRA:-}" | sed -n 's/.*path=\([^, ]*\).*/\1/p')
if [ -n "$CAP" ]; then
  mon quit
  for ((k = 0; k < 40; k++)); do
    echo "info status" | socat - "UNIX-CONNECT:${MON_SOCK:-$(cat "$OUT/mon.path")}" >/dev/null 2>&1 || break
    sleep 0.5
  done
  sleep 1
fi
finish
