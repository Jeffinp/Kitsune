# W60: YouTube's embedded player (Big Buck Bunny) in the browser, over the real network.
#
#   setsid nohup python3 tools/w24-site.py 8479 > /tmp/site-yt.log 2>&1 &     (the launch page /yt)
#   OUT=/tmp/w60; QEMU_MEM=256M QEMU_NIC=virtio-net-pci PORT=8479 \
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
#     bash tools/perf/run.sh IMG bios $OUT 900 tools/perf/scen/w60-youtube.sh
#
# The embed is opened from a page (/yt of tools/w24-site.py, which follows its link from a script):
# YouTube answers an embed opened with no Referer with "Error 153". Env: VIDEO (the id), PARAMS
# (query of the embed, default autoplay=1), WAIT (seconds to watch after the page is ready, default
# 240), CLICK=0 to never click the player.
#
# The verdict ($OUT/verdict.txt) says, stage by stage, how far it went: the HTML, the scripts, the
# player's API request, the media requests to googlevideo.com, the player in the page, the bytes
# appended to the source buffers, playback, pictures changing on screen, sound in the WAV. In a sandbox
# whose HTTPS is re-signed by a proxy the image needs the proxy's root (tools/gen-trust-store.sh with
# EXTRA_PEM: development only, never committed); the network of such a sandbox may also make YouTube
# refuse the video (docs/design/tls-browser.md §16), which the verdict reports as it is.
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8479}
VIDEO=${VIDEO:-aqz-KE-bpKQ}
PARAMS=${PARAMS:-autoplay=1}
WAIT=${WAIT:-240}
wait_for() { # wait_for PATTERN SECONDS
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
open_url() { key ctrl-l; sleep 0.3; typestr "$1"; key ret; }

wait_first_frame
dock_icon browser; click; sleep 3
open_url "http://203.0.113.5:$PORT/yt?go=1&v=$VIDEO&p=$PARAMS"
wait_for "fetch: GET https://www.youtube.com/embed/" 60
wait_for "handed to pid" 240
shot yt-0-scripts
wait_for "jsrun: page ready" 240
t_ready=$(date +%s)
shot yt-1-ready
# The page's box on screen: "page at X,Y size WxH" (the player fills it).
geo=$(grep -a "handed to pid" "$OUT/serial.log" | tail -1 | sed -n 's/.*page at \([0-9]*\),\([0-9]*\) size \([0-9]*\)x\([0-9]*\).*/\1 \2 \3 \4/p')
set -- $geo
PX=${1:-0}; PY=${2:-60}; PW=${3:-1280}; PH=${4:-600}
CXP=$(( PX + PW / 2 )); CYP=$(( PY + PH / 2 ))
clicked=0
i=2
while [ $(( $(date +%s) - t_ready )) -lt "$WAIT" ]; do
  sleep 15
  snap "yt-$i"
  i=$((i + 1))
  # Not playing a minute after the page is ready: a click in the middle of the player (what a person
  # does), once.
  if [ "${CLICK:-1}" = 1 ] && [ $clicked = 0 ] && [ $(( $(date +%s) - t_ready )) -ge 60 ] \
    && ! grep -aq "media: element [0-9]* playing at" "$OUT/serial.log"; then
    goto "$CXP" "$CYP"; click; clicked=1
    echo "clicked the player at $CXP,$CYP" > "$OUT/click.txt"
  fi
  # Two pictures one second apart while it plays.
  if grep -aq "media: element [0-9]* playing at" "$OUT/serial.log" && [ ! -f "$OUT/p1.ppm" ] && [ ! -f "$OUT/p1.png" ]; then
    sleep 4; snap p1; sleep 1; snap p2
  fi
done
flush_snaps
shot yt-final

CAP=$(printf '%s\n' "${QEMU_EXTRA:-}" | sed -n 's/.*path=\([^, ]*\).*/\1/p')
if [ -n "$CAP" ]; then
  mon quit
  for ((k = 0; k < 40; k++)); do
    echo "info status" | socat - "UNIX-CONNECT:${MON_SOCK:-$(cat "$OUT/mon.path")}" >/dev/null 2>&1 || break
    sleep 0.5
  done
  sleep 1
fi

{
  S="$OUT/serial.log"
  ok=1
  stage() { # stage NAME PATTERN
    if grep -aq -- "$2" "$S"; then echo "PASS $1"; else echo "FAIL $1"; ok=0; fi
  }
  stage "HTML of the embed loaded" "fetch: GET https://www.youtube.com/embed/"
  stage "the player's script (base.js) downloaded" "js: https://www.youtube.com/s/player/.*base.js -> "
  stage "the scripts handed to the page process" "handed to pid"
  stage "the scripts ran (page ready)" "jsrun: page ready"
  stage "scripts added by the page loaded (BotGuard, modules)" "jsfetch: [0-9]* GET https://www.google.com/js/th/"
  stage "the player asked the API (youtubei/v1/player)" "jsfetch: [0-9]* POST https://www.youtube.com/youtubei/v1/player"
  stage "the player asked googlevideo.com for the video" "jsfetch: [0-9]* POST https://[^ ]*googlevideo.com/videoplayback"
  stage "a <video> player in the browser (MediaSource attached)" "browser: player for element"
  stage "media bytes reached the source buffers" "media: element [0-9]* source [0-9]*: [0-9]* bytes appended"
  stage "playback started" "media: element [0-9]* playing at"
  if [ -f "$OUT/p1.png" ] && [ -f "$OUT/p2.png" ]; then
    d=$(compare -metric AE "$OUT/p1.png" "$OUT/p2.png" null: 2>&1 | cut -d' ' -f1)
    if [ "${d%.*}" -gt 2000 ] 2>/dev/null; then echo "PASS pictures change on screen ($d pixels differ in 1 s)"; else echo "FAIL pictures change on screen ($d)"; ok=0; fi
  else
    echo "FAIL pictures change on screen (never playing: no pair of pictures)"; ok=0
  fi
  if [ -n "$CAP" ] && [ -f "$CAP" ]; then
    python3 -I - "$CAP" <<'PY' || ok=0
import sys, wave, struct
w = wave.open(sys.argv[1])
rate, ch, n = w.getframerate(), w.getnchannels(), w.getnframes()
raw = w.readframes(n)
n = len(raw) // (2 * ch)
loud = 0
peak = 0
for i in range(0, n, 48):
    s = struct.unpack_from('<h', raw, i * 2 * ch)[0]
    if abs(s) > 40:
        loud += 48
    peak = max(peak, abs(s))
secs = loud / rate
print(("PASS" if secs >= 2 else "FAIL") + " sound in the recording: %.1f s not silent of %.1f s, peak %d" % (secs, n / rate, peak))
sys.exit(0 if secs >= 2 else 1)
PY
  else
    echo "SKIP sound (no QEMU_EXTRA audio capture)"
  fi
  echo "---- the page and the network (from the serial log)"
  grep -a "jsrun: .* ran in\|jsrun: page ready\|handed to pid\|js: https://www.youtube.com\|fetch: GET https://www.youtube.com/embed" "$S" | tr -d '\r' | sed 's/^.*\] //' | head -20
  grep -a "jsfetch: [0-9]* \(GET\|POST\)\|jsfetch: [0-9]* done\|jsfetch: [0-9]* failed\|cors: " "$S" | tr -d '\r' | sed 's/^.*\] //' | grep -v "\.png\|\.svg\|\.woff" | head -60
  echo "---- media"
  grep -a "media: \|browser: player\|browser: media" "$S" | tr -d '\r' | sed 's/^.*\] //' | head -40
  echo "---- page console"
  grep -a "\[jsrun:" "$S" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' | head -40
  [ -f "$OUT/click.txt" ] && cat "$OUT/click.txt"
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
