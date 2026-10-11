# W45: cost of driving v2 apps (paint strokes, notes typing, clock ticks); read the `[trace] wasm:` line.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
open_app() { key ctrl-spc; sleep 1.2; typestr "$1"; sleep 0.8; key ret; sleep 3; }
sleep 2
open_app pintura
goto 450 400
mon "mouse_button 1"; sleep 0.1
for i in $(seq 1 80); do move 4 3; sleep 0.04; done
for i in $(seq 1 80); do move -4 -3; sleep 0.04; done
mon "mouse_button 0"; sleep 0.5
open_app notas; typestr "the quick brown fox jumps over the lazy dog"; sleep 1
open_app relogio; sleep 8
shot a-end
sleep 5
finish
