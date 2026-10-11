# W46: ring 3 programs with files: arguments, the working folder, a pipe into stdin, the user's rights.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "echo hello-from-a-file > note.txt"; key ret; sleep 1
typestr "run cat note.txt"; key ret; sleep 3
typestr "echo piped text | run cat"; key ret; sleep 3
typestr "run ls /"; key ret; sleep 3
typestr "run cat /etc/accounts"; key ret; sleep 3
typestr "run cat missing.txt"; key ret; sleep 3
typestr "run fstest"; key ret; sleep 8
shot a-terminal
finish
