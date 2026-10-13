# W56: sound through the AC'97 driver, proved on the WAV QEMU records. Run with the device and the
# capture (the path must be absolute; the verdict reads the same path from QEMU_EXTRA):
#
#   OUT=/tmp/w56; QEMU_MEM=256M \
#   QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
#   bash tools/perf/run.sh IMG bios $OUT 400 tools/perf/scen/w56-audio.sh
#
# In the Terminal: a 440 Hz tone for 2 s; a WAV generated in the guest (660 Hz, 22.05 kHz mono 16 bits:
# resampled to 48 kHz) and played; another (550 Hz, 44.1 kHz stereo 8 bits); a chord (440 + 660 Hz, two
# streams at once); the system volume at 25 % (a tone at a quarter of the level); a stream at 50 %
# volume; a tone with the stream clock printed (`--clock`); a beep. The verdict ($OUT/verdict.txt)
# measures each sound in the capture (frequency by FFT, duration, level, purity, silence after it) with
# tools/perf/audio-check.py, checks that the clock follows the system clock, and that no player
# reported an underrun. The scenario quits QEMU itself before the verdict (QEMU writes the WAV header
# on exit), so the picture is a-terminal.png, not screen.png.
source "$(dirname "$0")/../lib.sh"
count() { grep -ac -- "$1" "$OUT/serial.log" 2>/dev/null || true; }
wait_n() { # wait_n PATTERN N SECONDS: until serial.log has N lines with the pattern
  local i
  for ((i = 0; i < $3 * 2; i++)); do [ "$(count "$1")" -ge "$2" ] && return 0; sleep 0.5; done
  return 1
}
cmd() { typestr "$1"; key ret; }
DONE="\] tocado: "
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
cmd "tone 440 2000";                          wait_n "$DONE" 1 60; sleep 1
cmd "play --gen t22.wav 660 1000 22050 1 16"; wait_n "quadros gravados" 1 60
cmd "play t22.wav";                           wait_n "$DONE" 2 60; sleep 1
cmd "play --gen s44.wav 550 800 44100 2 8";   wait_n "quadros gravados" 2 60
cmd "play s44.wav";                           wait_n "$DONE" 3 60; sleep 1
cmd "tone 440,660 1500";                      wait_n "$DONE" 5 60; sleep 1
cmd "volume 25";                              wait_n "volume: 25%" 1 30
cmd "tone 440 1000";                          wait_n "$DONE" 6 60; sleep 1
cmd "volume 100";                             wait_n "volume: 100%" 1 30
cmd "tone 880 600 -v 50";                     wait_n "$DONE" 7 60; sleep 1
cmd "tone 330 1500 --clock";                  wait_n "$DONE" 8 60; sleep 1
cmd "beep";                                   wait_n "$DONE" 9 60
sleep 3
shot a-terminal
# Ajustes, section Som (the last one: Up from the first wraps to it).
dock_icon settings; click; sleep 2.5
key up; sleep 1.5
shot b-ajustes-som
# QEMU completes the WAV (its header) only when it quits: quit now, wait for the monitor to go away.
mon quit
for ((i = 0; i < 40; i++)); do
  echo "info status" | socat - "UNIX-CONNECT:${MON_SOCK:-$(cat "$OUT/mon.path")}" >/dev/null 2>&1 || break
  sleep 0.5
done
sleep 1

CAP=$(printf '%s\n' "${QEMU_EXTRA:-}" | sed -n 's/.*path=\([^, ]*\).*/\1/p')
{
  ok=1
  need() { if grep -aq -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  need "audio: AC'97 up" "AC'97 driver up"
  need "\[play:[0-9]*\] t22.wav: 22050 Hz, 1 canal(is), 16 bits, 1000 ms" "WAV 22.05 kHz mono 16-bit parsed"
  need "\[play:[0-9]*\] s44.wav: 44100 Hz, 2 canal(is), 8 bits, 800 ms" "WAV 44.1 kHz stereo 8-bit parsed"
  n=$(grep -a "$DONE" "$OUT/serial.log" | grep -vc " 0 falha(s)")
  if [ "$n" = 0 ]; then echo "PASS no player reported an underrun"; else echo "MISS $n player(s) reported underruns"; ok=0; fi
  if grep -aq "device starved" "$OUT/serial.log"; then echo "MISS the device starved"; ok=0; else echo "PASS the device never starved"; fi
  grep -a "$DONE\|clock uptime" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\] //'
  # The plan: 440 Hz 2 s; 660 Hz 1 s (from 22.05 kHz); 550 Hz 0.8 s (from 44.1 kHz 8-bit); the chord;
  # 440 Hz at 25 % master; 880 Hz at 50 % stream volume; 330 Hz 1.5 s (clock); the beep (880 Hz 150 ms).
  if [ -n "$CAP" ] && [ -f "$CAP" ]; then
    python3 -I "$(dirname "$0")/../audio-check.py" "$CAP" \
      "440:2000:1;660:1000:1;550:800;440+660:1500;440:1000:0.25;880:600:0.5;330:1500:1;880:150" \
      --serial "$OUT/serial.log" || ok=0
  else
    echo "MISS no capture file (QEMU_EXTRA needs -audiodev wav,...,path=...)"; ok=0
  fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
