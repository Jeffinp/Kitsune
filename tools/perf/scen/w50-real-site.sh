# W50: real sites over the real network (needs outbound access from QEMU's user networking).
# Env: SITES (space separated URLs), WAIT (seconds per page), CONTINUE=1 clicks "continue anyway" on a
# certificate error page (a sandbox whose egress proxy re-signs HTTPS has no chain the browser trusts).
source "$(dirname "$0")/../lib.sh"
SITES=${SITES:-"http://example.com https://example.com"}
WAIT=${WAIT:-15}
wait_first_frame
dock_icon browser; click; sleep 3
i=0
for u in $SITES; do
  i=$((i+1))
  key ctrl-l; sleep 0.3
  typestr "$u"
  key ret
  sleep "$WAIT"
  if [ "${CONTINUE:-0}" = 1 ]; then
    goto 608 486; click; sleep "$WAIT"
  fi
  shot "site-$i"
  key pgdn; sleep 1
  shot "site-$i-2"
done
finish
