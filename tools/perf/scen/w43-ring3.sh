# W43: ring 3 programs: run, fault containment, a spinning process killed. UEFI, 1280x800.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "run hello"; key ret; sleep 3
typestr "run probe"; key ret; sleep 2
typestr "run crash"; key ret; sleep 2
typestr "ps"; key ret; sleep 1
shot a-terminal
finish
