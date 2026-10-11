# W44: v2 WebAssembly apps inside ring 3 host processes (notes, paint, clock) + `ps`. UEFI, 1280x800.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
open_app() { key ctrl-spc; sleep 1.2; typestr "$1"; sleep 0.8; key ret; sleep 3; }
sleep 2
open_app notas; typestr "ola ring 3"; sleep 1.5; shot a-notes
open_app pintura; sleep 2; shot b-paint
open_app relogio; sleep 3; shot c-clock
goto 603 202; click; sleep 2               # close the clock: its process ends and its frames are freed
open_app terminal; sleep 1; typestr "ps"; key ret; sleep 1.5; shot d-ps
typestr "kill 5000"; key ret; sleep 2     # the notes host dies: only the Notas window shows an error
typestr "ps"; key ret; sleep 1.5; shot e-after-kill
finish
