# W47: a ring 3 program with a window of its own: frames, pointer, keys, resize, closing.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "run gfxdemo"; key ret; sleep 5
shot a-window
goto 520 330; sleep 0.5; key k; sleep 1
shot b-input
goto 567 146; click; sleep 3           # the close button: the program is told to goto 150 250; click; sleep 0.5
typestr "run gfxdemo"; key ret; sleep 5     # again: now stop it from the terminal (Ctrl+C)
goto 150 250; click; sleep 0.5; key ctrl-c; sleep 3   # (the window no longer has the focus)
shot d-killed
finish
shot c-closed
goto 150 250; click; sleep 0.5
typestr "run gfxdemo"; key ret; sleep 5     # again: now stop it from the terminal (Ctrl+C)
goto 150 250; click; sleep 0.5; key ctrl-c; sleep 3   # (the window no longer has the focus)
shot d-killed
finish
