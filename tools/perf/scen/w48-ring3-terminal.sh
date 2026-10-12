# W48: the Terminal as a ring 3 program (`run kterm`): its own window, the shell engine over the file
# and process calls, a child program, Ctrl+C on a command that waits, closing it.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "run kterm"; key ret; sleep 6
shot a-open
typestr "ls /"; key ret; sleep 3
shot b-ls
typestr "echo oi > /kt.txt"; key ret; sleep 2     # the root folder is not the user's: refused
typestr "echo oi > kt.txt"; key ret; sleep 2
typestr "cat kt.txt"; key ret; sleep 2
typestr "run hello"; key ret; sleep 4
typestr "date"; key ret; sleep 2
shot c-run
typestr "sleep 30"; key ret; sleep 2
key ctrl-c; sleep 3
typestr "echo depois"; key ret; sleep 2
shot d-ctrlc
goto 1207 138; click; sleep 3     # the close button: the program is told to finish
shot e-closed
finish
