# W52: JavaScript in the browser. The page /js of tools/w24-site.py has an external script (/js/app.js)
# and an inline one: they change the title, the heading and a paragraph, build a list with
# createElement/appendChild/insertAdjacentHTML, start a setTimeout, a setInterval and an animation
# frame. The scenario clicks the page's button twice (a counter and a class that changes the
# background), a link whose handler calls preventDefault (the page must stay), a `javascript:` link,
# a button whose handler loops forever (the task is stopped after 3 s, the desktop and the page keep
# working) and the counter again. The verdict is in $OUT/verdict.txt, from the lines the page's script
# process printed (`[jsrun:PID] ...` in serial.log) and the browser's log; the screenshots show the
# page before and after.
#   tools/w24-site.py &
#   QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     tools/perf/run.sh <img> bios <out> 400 tools/perf/scen/w52-browser-js.sh
# The page coordinates below are for BIOS (1280x720) with the browser window where the dock opens it.
source "$(dirname "$0")/../lib.sh"
wait_for() { # wait_for PATTERN SECONDS
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "http://203.0.113.5:8079/js"
key ret
wait_for "jsrun: page ready" 120
sleep 1
wait_for "js: interval done" 60
sleep 2
shot a-loaded
# The button "Clique aqui", twice.
goto "${BTN_X:-219}" "${BTN_Y:-389}"; click
wait_for "js: click 1 " 30
sleep 2
shot b-click1
click
wait_for "js: click 2 " 30
sleep 2
# The link whose handler calls preventDefault: the page must stay.
goto "${LINK_X:-250}" "${LINK_Y:-536}"; click
wait_for "js: mark link segurado" 30
sleep 3
shot c-prevented
# The `javascript:` link.
goto "${JSLINK_X:-415}" "${JSLINK_Y:-536}"; click
wait_for "js: mark js-link" 30
sleep 2
# The endless loop: stopped after 3 s; the desktop keeps drawing meanwhile.
goto "${LOOP_X:-329}" "${LOOP_Y:-389}"; click
wait_for "js: entering an endless loop" 30
sleep 1
snap d-during-loop
wait_for "ran for too long and was stopped" 60
sleep 1
goto "${BTN_X:-219}" "${BTN_Y:-389}"; click
wait_for "js: click 3 " 30
sleep 2
shot e-after-loop
flush_snaps

{
  grep -a "\[jsrun:" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' > "$OUT/js-out.txt"
  ok=1
  need() { if grep -aq -- "$1" "$OUT/js-out.txt"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needlog() { if grep -aq -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needlog "js: http://203.0.113.5:8079/js/app.js -> " "external script fetched"
  needlog "browser: 2 script(s) handed to pid" "script process started"
  need "^js: app.js start, ua=Mozilla/5.0 host=203.0.113.5:8079 path=/js" "location and navigator"
  need "^js: inline sees app = object readyState loading" "scripts run in order"
  need "^js: DOMContentLoaded, items before: 0" "DOMContentLoaded"
  need "^js: list has 4 items; matches true closest list" "createElement, appendChild, innerHTML, selectors"
  need "^js: load event" "load"
  need "^js: microtask" "promise jobs"
  need "^js: timeout fired" "setTimeout"
  need "^js: interval done" "setInterval"
  need "^js: animation frame true" "requestAnimationFrame"
  need "^js: click 1 target btn class box on" "click reaches the button (1)"
  need "^js: click 2 target btn class box$" "click reaches the button (2)"
  need "^js: mark link segurado" "preventDefault on a link"
  need "^js: mark js-link" "javascript: link"
  need "^Uncaught InternalError: interrupted" "endless loop interrupted"
  need "ran for too long and was stopped" "the task is stopped"
  need "^js: click 3 target btn" "the page works after the loop"
  if grep -aq "fetch: GET http://203.0.113.5/type " "$OUT/serial.log"; then
    echo "MISS the prevented link was followed"; ok=0
  else
    echo "PASS the prevented link was not followed"
  fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
