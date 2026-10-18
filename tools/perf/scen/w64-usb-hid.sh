# W64: USB keyboard and mouse (xHCI) reach the desktop. Needs the controller and no PS/2:
#   QEMU_EXTRA="-machine pc,i8042=off -device qemu-xhci -device usb-kbd -device usb-mouse" \
#     bash tools/perf/run.sh IMG bios $OUT 150 tools/perf/scen/w64-usb-hid.sh
# Ctrl+Space opens Busca and Esc closes it (keyboard), a move to the dock's Calculator icon and a click opens the
# Calculator (mouse), and typing reaches it. (The desktop starts with a Terminal open, so the Terminal icon would
# minimise it.) Each step is judged by how many pixels of the screen changed
# (ImageMagick `compare -metric AE`). tools/qemu-usb-hid.ps1 is the same scenario for Windows.
source "$(dirname "$0")/../lib.sh"
changed() { compare -metric AE "$OUT/$1.png" "$OUT/$2.png" null: 2>&1 | grep -o '^[0-9]*' | head -1; }
verdict() { # verdict <pixels> <min> <text>
  if [ "${1:-0}" -gt "$2" ]; then echo "PASS $3 ($1 pixels changed)" >> "$OUT/w64-result.txt"; else echo "FAIL $3 (${1:-0} pixels changed)" >> "$OUT/w64-result.txt"; fi
}
wait_first_frame
: > "$OUT/w64-result.txt"
if grep -aq "usb: slot [0-9]*: boot keyboard" "$OUT/serial.log" && grep -aq "usb: slot [0-9]*: boot mouse" "$OUT/serial.log"; then
  echo "PASS the guest found a USB keyboard and a USB mouse" >> "$OUT/w64-result.txt"
else
  echo "FAIL the guest found a USB keyboard and a USB mouse" >> "$OUT/w64-result.txt"
fi
shot a-idle
key ctrl-spc; sleep 2
shot b-search
verdict "$(changed a-idle b-search)" 5000 "Ctrl+Space on the USB keyboard opened Busca"
key esc; sleep 2
shot c-closed
verdict "$(changed b-search c-closed)" 5000 "Esc on the USB keyboard closed it"
dock_icon calc; click; sleep 4
shot d-calculator
verdict "$(changed c-closed d-calculator)" 5000 "moving the USB mouse and clicking opened the Calculator"
typestr "123"; sleep 1
shot e-typed
verdict "$(changed d-calculator e-typed)" 100 "typing on the USB keyboard reached the Calculator"
grep -aq "usb: key 0x1d down" "$OUT/serial.log" && echo "PASS the log shows the Ctrl key press" >> "$OUT/w64-result.txt" || echo "FAIL the log shows the Ctrl key press" >> "$OUT/w64-result.txt"
grep -aq "usb: mouse dx=[0-9]* dy=-\?[0-9]* left=true" "$OUT/serial.log" && echo "PASS the log shows the click" >> "$OUT/w64-result.txt" || echo "FAIL the log shows the click" >> "$OUT/w64-result.txt"
finish
