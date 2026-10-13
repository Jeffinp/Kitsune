# W56 (no device): the system boots and works without a sound card. Run WITHOUT the AC'97 in QEMU_EXTRA:
#
#   QEMU_MEM=256M bash tools/perf/run.sh IMG bios /tmp/w56n 200 tools/perf/scen/w56-audio-nodev.sh
#
# The log says `audio: no device`, the sound commands answer that there is no device (no hang, no crash)
# and the Terminal keeps working afterwards. Verdict in $OUT/verdict.txt.
source "$(dirname "$0")/../lib.sh"
wait_for() { # wait_for PATTERN SECONDS: until serial.log has the pattern
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "tone 440 300"; key ret
wait_for "dispositivo de som" 60
typestr "volume"; key ret
sleep 4
typestr "run hello"; key ret
wait_for "hello from" 60
sleep 2
shot a-nodev
{
  ok=1
  need() { if grep -aq -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  need "audio: no device" "boot log says there is no sound device"
  need "\[play:[0-9]*\] play: não há dispositivo de som" "tone answers that there is no device"
  [ "$(grep -ac "não há dispositivo de som" "$OUT/serial.log")" -ge 2 ] && echo "PASS volume too" || { echo "MISS volume too"; ok=0; }
  need "hello from" "a program runs afterwards"
  if grep -aq "audiod" "$OUT/serial.log"; then echo "MISS audiod started without a device"; ok=0; else echo "PASS no audiod thread"; fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
