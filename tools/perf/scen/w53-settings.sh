# W53: the global switch in Ajustes > Rede > Navegador. Turn the blocker off there, then open /ads: the page
# loads without the filter (the shield shows crossed out and its switch is dim), and `adblock=0` is saved.
# Same server and network setup as w53-shields.sh.
source "$(dirname "$0")/../lib.sh"
SITE=${SITE:-http://203.0.113.5:8079}
NET_X=${NET_X:-250}; NET_Y=${NET_Y:-332}
SW_X=${SW_X:-700}; SW_Y=${SW_Y:-598}
wait_first_frame
dock_icon settings; click; sleep 3
shot s-open
goto "$NET_X" "$NET_Y"; click; sleep 1.5
goto 730 400
for _ in 1 2 3 4 5 6 7 8 9 10; do mon "mouse_move 0 0 -1"; sleep 0.15; done; sleep 1
shot s-network
goto "$SW_X" "$SW_Y"; click; sleep 1
shot s-toggled
key ctrl-w; sleep 1
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "$SITE/ads"
key ret
sleep 35
shot s-page
: > "$OUT/w53-result.txt"
if grep -aq "shields: page 203.0.113.5 on=false ads=0 trackers=0 hidden=0 total=0" "$OUT/serial.log"; then echo "PASS global off: the page opened without the filter" >> "$OUT/w53-result.txt"; else echo "FAIL global off: the filter was still on" >> "$OUT/w53-result.txt"; fi
finish
