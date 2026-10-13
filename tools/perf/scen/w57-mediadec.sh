# W57: WebM (VP9 + Opus) decoding in ring 3 (`run mediadec`). The test files are fetched from the
# host with `wget` (a small HTTP server on port 8857 serves $MEDIA), then decoded with --md5: every
# picture's MD5 must equal ffmpeg's (libvpx) framemd5 and the audio's MD5 must equal the PCM that
# the same fixed-point decoder gave on the host (tools/media/check-host.sh, where it is compared with
# ffmpeg: SNR > 40 dB, same sample count). Also: the message server in memory (--serve-test), a file
# cut in the middle and one with corrupted clusters (no crash, the system goes on), --info, and the
# video played in a window (--show, screenshot d-show). The verdict is in $OUT/verdict.txt, with the
# decoding speed of each size under emulation.
#
# Prepare MEDIA first (host): bash tools/media/gen-test-media.sh $M && bash tools/media/check-host.sh $M
# Run: MEDIA=$M QEMU_MEM=256M bash tools/perf/run.sh IMG bios OUT 1500 tools/perf/scen/w57-mediadec.sh
source "$(dirname "$0")/../lib.sh"
MEDIA=${MEDIA:?set MEDIA to the folder made by tools/media/gen-test-media.sh and check-host.sh}
PORT=${MEDIA_PORT:-8857}
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$MEDIA" > "$OUT/http.log" 2>&1 &
http_pid=$!
trap 'kill $http_pid 2>/dev/null' EXIT
wait_for() { # wait_for PATTERN SECONDS: until serial.log has the pattern
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
count() { grep -ac -- "$1" "$OUT/serial.log"; }
wait_count() { # wait_count PATTERN N SECONDS: until the pattern appears N times
  local i
  for ((i = 0; i < $3 * 2; i++)); do [ "$(count "$1")" -ge "$2" ] && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
for f in v180 v360 v720 cut bad; do
  typestr "wget http://10.0.2.2:$PORT/$f.webm"; key ret; sleep 6
done
typestr "ls -l"; key ret; sleep 1
shot a-files
n=0
for f in v180 v360 v720; do
  n=$((n + 1))
  typestr "run mediadec $f.webm --md5"; key ret
  wait_count "\] audio md5 " $n 1200
  sleep 2
done
shot b-decoded
typestr "run mediadec --serve-test v180.webm"; key ret
wait_for "serve-test: \(PASS\|FAIL\)" 600
typestr "run mediadec cut.webm"; key ret
wait_count "\] total: " 4 600
typestr "run mediadec bad.webm"; key ret
wait_count "\] total: " 5 600
sleep 2
typestr "run mediadec v360.webm --info"; key ret
wait_count "\] mediadec: v360.webm: " 2 120
typestr "run mediadec v360.webm --rgb 1280x720 --frames 30"; key ret
wait_count "\] rgba 1280x720: " 1 600
sleep 2
shot c-hostile
typestr "run mediadec v360.webm --show --frames 90"; key ret
wait_count "\] mediadec: v360.webm: " 4 120
sleep 6
shot d-show
wait_count "\] total: " 7 900
sleep 3
shot e-end

{
  grep -a "\[mediadec:" "$OUT/serial.log" | tr -d '\r' > "$OUT/mediadec-out.txt"
  ok=1
  pass() { echo "PASS $1"; }
  miss() { echo "MISS $1"; ok=0; }
  # The process id whose output names FILE (first run of it).
  pid_of() { grep -a "\] mediadec: $1: " "$OUT/mediadec-out.txt" | head -1 | sed 's/^.*\[mediadec:\([0-9]*\)\].*/\1/'; }
  lines_of() { grep -a "\[mediadec:$1\] " "$OUT/mediadec-out.txt" | sed 's/^.*\[mediadec:[0-9]*\] //'; }
  for f in v180 v360 v720; do
    pid=$(pid_of "$f.webm")
    if [ -z "$pid" ]; then miss "$f: no run found"; continue; fi
    lines_of "$pid" > "$OUT/$f.out"
    grep -a "^frame " "$OUT/$f.out" | awk '{print $5}' > "$OUT/$f.got"
    grep -v '^#' "$MEDIA/$f.vmd5" | awk -F, '{gsub(/ /, "", $6); print $6}' > "$OUT/$f.want"
    if [ -s "$OUT/$f.want" ] && cmp -s "$OUT/$f.got" "$OUT/$f.want"; then
      pass "$f: $(wc -l < "$OUT/$f.got") pictures, MD5 identical to ffmpeg's framemd5"
    else
      miss "$f: picture MD5s ($(wc -l < "$OUT/$f.got") got, $(wc -l < "$OUT/$f.want") wanted, first diff line $(cmp "$OUT/$f.got" "$OUT/$f.want" 2>&1 | awk '{print $NF}'))"
    fi
    amd5=$(grep -a "^audio md5 " "$OUT/$f.out" | awk '{print $3}')
    asamples=$(grep -a "^audio md5 " "$OUT/$f.out" | sed 's/.*(\([0-9]*\) samples.*/\1/')
    if [ "$amd5" = "$(cat "$MEDIA/$f.pcm.md5" 2>/dev/null)" ] && [ "$asamples" = 144000 ]; then
      pass "$f: audio MD5 identical to the host decode (144000 samples per channel)"
    else
      miss "$f: audio md5 $amd5 ($asamples samples) vs host $(cat "$MEDIA/$f.pcm.md5" 2>/dev/null)"
    fi
    grep -a "^video: \|^audio: \|^total: " "$OUT/$f.out" | sed "s/^/  $f /"
  done
  if grep -aq "serve-test: PASS" "$OUT/mediadec-out.txt"; then pass "serve-test"; else miss "serve-test"; fi
  grep -a "serve-test: " "$OUT/mediadec-out.txt" | sed 's/^.*\] /  /'
  for f in cut bad; do
    pid=$(pid_of "$f.webm")
    if [ -n "$pid" ] && lines_of "$pid" | grep -aq "^total: "; then
      pass "$f.webm ends normally: $(lines_of "$pid" | grep -a '^video: ')"
    else
      miss "$f.webm"
    fi
  done
  bad=$(pid_of bad.webm)
  [ -n "$bad" ] && echo "  bad.webm: $(lines_of "$bad" | grep -ac 'did not decode') packets did not decode"
  if [ "$(grep -ac '\] mediadec: v360.webm: ' "$OUT/mediadec-out.txt")" -ge 4 ]; then pass "runs after the hostile files (--info, --rgb, --show)"; else miss "later runs"; fi
  grep -a "rgba 1280x720" "$OUT/mediadec-out.txt" | sed 's/^.*\] /  /'
  if grep -aq "video: 90 pictures 640x360" "$OUT/mediadec-out.txt"; then pass "--show played 90 pictures"; else miss "--show"; fi
  # Every mediadec run must end by itself (exit), never be killed by a fault; the kernel never panics.
  grep -a "proc: 'mediadec' pid [0-9]* \(exited\|was killed\)" "$OUT/serial.log" | sed 's/^.*proc: /  /'
  if grep -aq "proc: 'mediadec' pid [0-9]* was killed\|panic" "$OUT/serial.log"; then
    grep -a "proc: 'mediadec' pid [0-9]* was killed\|panic" "$OUT/serial.log" | head -3
    miss "a mediadec run was killed or the kernel panicked"
  else
    pass "every mediadec run exited by itself"
  fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
