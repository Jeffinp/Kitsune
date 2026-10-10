# W39: boot with two accounts: the login screen. Wrong password, then the right one, for maria.
# Run with FS_IMG = the disk W38 left behind. UEFI, 1280x800.
source "$(dirname "$0")/../lib.sh"
sleep 24
shot a-login
key tab; sleep 0.5
typestr "errada1"; key ret; sleep 2
shot b-wrong
for i in 1 2 3 4 5 6 7; do key backspace; done
sleep 3; typestr "senha1"; key ret; sleep 4
shot c-desktop
finish
