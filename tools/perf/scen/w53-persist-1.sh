# W53 persistence, boot 1: turn the ad blocker off for one site with the shield's switch. Boot 2
# (w53-persist-2.sh) reuses the disk image: FS_IMG=<outdir of boot 1>/fs.img. Same setup as w53-shields.sh.
source "$(dirname "$0")/../lib.sh"
SITE=${SITE:-http://203.0.113.5:8079}
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "$SITE/ads"
key ret
sleep 25
goto 960 116; click; sleep 1
goto 800 224; click
sleep 30
shot p1-off
sleep 5
if grep -aq "shields: off for 203.0.113.5" "$OUT/serial.log"; then echo "PASS boot 1: the site was turned off" > "$OUT/w53-result.txt"; else echo "FAIL boot 1: the site was not turned off" > "$OUT/w53-result.txt"; fi
finish
