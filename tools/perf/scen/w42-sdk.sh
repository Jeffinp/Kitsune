# W42: the SDK tutorial's last steps: wget a .wasm built on the host, open it from Arquivos (it is
# installed and run). Needs `python3 -m http.server 8000` in the folder with hello.wasm. UEFI, 1280x800.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "wget http://10.0.2.2:8000/hello.wasm"; key ret; sleep 5
typestr "ls -l hello.wasm"; key ret; sleep 0.8
shot a-terminal
dock_icon files; click; sleep 2
goto 480 288; click; sleep 0.4; key ret; sleep 3
shot b-running
finish
