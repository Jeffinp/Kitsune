# W53: the native ad and tracker blocker. The page /ads of tools/w24-site.py asks for one style sheet,
# three pictures and two pixels that the rules catch, two pictures from reserved .invalid hosts and an ad box
# that holds one more picture. Phases: (A) blocker on by default, (B) the shield's popover, (C) the per-site
# switch turned off (the page reloads without the filter), (D) switched on again.
#
#   tools/w24-site.py 2>> /tmp/w53-requests.log &
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     REQLOG=/tmp/w53-requests.log tools/perf/run.sh <img> bios <out> 260 tools/perf/scen/w53-shields.sh
#
# Writes $OUT/w53-result.txt with PASS/FAIL lines (what the guest's serial log and the server's request log
# say) next to the screenshots a-on, b-popover-on, c-off, d-popover-off, e-on-again.
source "$(dirname "$0")/../lib.sh"
SITE=${SITE:-http://203.0.113.5:8079}
# Where the shield and the popover's switch row are (BIOS 1280x720; see the screenshots).
SHIELD_X=${SHIELD_X:-960}; SHIELD_Y=${SHIELD_Y:-116}
SWITCH_X=${SWITCH_X:-800}; SWITCH_Y=${SWITCH_Y:-224}
REQLOG=${REQLOG:-}
res() { echo "$@" >> "$OUT/w53-result.txt"; }
# count lines of the server's request log that match a path since the mark
mark() { wc -l < "$REQLOG" | tr -d " "; }
asked() { tail -n +"$(( $1 + 1 ))" "$REQLOG" | grep -a -c -- "$2" || true; }
expect() { # name condition-exit-status
  if [ "$2" = 0 ]; then res "PASS $1"; else res "FAIL $1"; fi
}
: > "$OUT/w53-result.txt"
wait_first_frame
dock_icon browser; click; sleep 3
M0=$(mark)
key ctrl-l; sleep 0.3
typestr "$SITE/ads"
key ret
sleep 25
shot a-on
grep -aq "shields: page 203.0.113.5 on=true ads=4 trackers=2 hidden=2 total=8" "$OUT/serial.log"; expect "A on: serial says ads=4 trackers=2 hidden=2 total=8" $?
if [ -n "$REQLOG" ]; then
  for p in "/ads/ad-theme.css" "/ads/banner-728.png" "/pagead/ad.png" "/tracking/pixel.png" "in-hidden-ad"; do
    [ "$(asked "$M0" "$p")" = 0 ]; expect "A on: server never saw $p" $?
  done
  [ "$(asked "$M0" "GET /ads ")" -ge 1 ]; expect "A on: the page itself was requested" $?
  [ "$(asked "$M0" "GET /g.png")" -ge 1 ]; expect "A on: the site's own picture was requested" $?
fi
[ "$(grep -ac "fetch: GET http://kitsune-" "$OUT/serial.log")" = 0 ]; expect "A on: the guest never tried the .invalid hosts" $?

# (B) the popover
goto "$SHIELD_X" "$SHIELD_Y"; click; sleep 1
shot b-popover-on

# (C) the switch off: the page reloads without the filter
M1=$(mark)
goto "$SWITCH_X" "$SWITCH_Y"; click
sleep 40
shot c-off
grep -aq "shields: off for 203.0.113.5" "$OUT/serial.log"; expect "C: the site was turned off" $?
grep -aq "shields: page 203.0.113.5 on=false ads=0 trackers=0 hidden=0 total=0" "$OUT/serial.log"; expect "C off: serial says nothing blocked" $?
if [ -n "$REQLOG" ]; then
  for p in "/ads/ad-theme.css" "/ads/banner-728.png" "/pagead/ad.png" "/tracking/pixel.png" "in-hidden-ad"; do
    [ "$(asked "$M1" "$p")" -ge 1 ]; expect "C off: server saw $p" $?
  done
fi
grep -aq "fetch: GET http://kitsune-ads.invalid/banner.png" "$OUT/serial.log"; expect "C off: the guest tried the .invalid ad host" $?
grep -aq "fetch: GET http://kitsune-tracker.invalid/pixel.gif" "$OUT/serial.log"; expect "C off: the guest tried the .invalid tracker host" $?
goto "$SHIELD_X" "$SHIELD_Y"; click; sleep 1
shot d-popover-off

# (D) on again
M2=$(mark)
goto "$SWITCH_X" "$SWITCH_Y"; click
sleep 25
shot e-on-again
grep -aq "shields: on for 203.0.113.5" "$OUT/serial.log"; expect "D: the site was turned back on" $?
[ "$(grep -ac "shields: page 203.0.113.5 on=true ads=4 trackers=2 hidden=2 total=8" "$OUT/serial.log")" -ge 2 ]; expect "D on: serial says the same totals again" $?
if [ -n "$REQLOG" ]; then
  [ "$(asked "$M2" "/ads/banner-728.png")" = 0 ]; expect "D on: server did not see the ad again" $?
fi
finish
