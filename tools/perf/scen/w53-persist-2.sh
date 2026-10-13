# W53 persistence, boot 2 (disk image of w53-persist-1.sh): the site is still off after a reboot, so the
# page opens without the filter, with no touch on the shield.
source "$(dirname "$0")/../lib.sh"
SITE=${SITE:-http://203.0.113.5:8079}
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "$SITE/ads"
key ret
sleep 35
shot p2-still-off
: > "$OUT/w53-result.txt"
if grep -aq "shields: page 203.0.113.5 on=false ads=0 trackers=0 hidden=0 total=0" "$OUT/serial.log"; then echo "PASS boot 2: the site is still off after the reboot" >> "$OUT/w53-result.txt"; else echo "FAIL boot 2: the site is not off" >> "$OUT/w53-result.txt"; fi
if grep -aq "css: http://203.0.113.5:8079/ads/ad-theme.css" "$OUT/serial.log"; then echo "PASS boot 2: the ad sheet was fetched (no filter)" >> "$OUT/w53-result.txt"; else echo "FAIL boot 2: the ad sheet was not fetched" >> "$OUT/w53-result.txt"; fi
finish
