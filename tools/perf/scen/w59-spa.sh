# W59: frameworks in the browser. The pages /preact (TodoMVC with Preact + htm) and /vue (Vue 3 with
# an in-DOM template) of tools/w24-site.py run with real typing and clicking through the QEMU monitor:
# the field gets the text typed, Enter adds a to-do, a check box marks it, the hash links filter
# (pushState: the address bar follows), the remove button removes, the list survives a reload
# (localStorage in ~/.webstorage); in Vue a counter, v-model, v-if, v-for, @keyup.enter, a check box,
# a <select> and a form with @submit.prevent. The pages print their state on the console after every
# change ("todo: ...", "vue: ...": the serial log) and where their controls are on screen ("... at
# name=x,y", from getBoundingClientRect after each layout, through a ResizeObserver); the scenario
# clicks there. Then /worker: a Web Worker (a process of its own) counts primes, fetches a file and
# answers the page. The verdict is in $OUT/verdict.txt.
#   tools/w24-site.py 8279 &
#   PORT=8279 QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     tools/perf/run.sh <img> bios <out> 900 tools/perf/scen/w59-spa.sh
# The pages must fit the browser window as it opens (BIOS, 1280x720): the scenario does not scroll.
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8079}
SITE="http://203.0.113.5:$PORT"
log() { grep -a -- "$1" "$OUT/serial.log" | tr -d '\r'; }
count() { grep -ac -- "$1" "$OUT/serial.log"; }
wait_for() { # wait_for PATTERN SECONDS
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  echo "timeout waiting for $1" >> "$OUT/steps.txt"
  return 1
}
wait_more() { # wait_more PATTERN COUNT SECONDS: until PATTERN shows more than COUNT times
  local i
  for ((i = 0; i < $3 * 2; i++)); do [ "$(count "$1")" -gt "$2" ] && return 0; sleep 0.5; done
  echo "timeout waiting for a new $1" >> "$OUT/steps.txt"
  return 1
}
# Where the page is on screen (the browser logs it when the page's scripts start).
origin() { log "handed to pid" | tail -1 | sed -E 's/.*page at (-?[0-9]+),(-?[0-9]+) (size [0-9]+x[0-9]+ )?zoom ([0-9]+).*/\1 \2 \4/'; }
# Click the control NAME of the last "PREFIX at ..." line the page printed.
click_at() {
  local ox oy z cx cy
  read -r ox oy z < <(origin)
  read -r cx cy < <(log "$1: at " | tail -1 | sed -E "s/.* $2=(-?[0-9]+),(-?[0-9]+).*/\1 \2/")
  echo "click $1 $2 at client $cx,$cy page origin $ox,$oy zoom $z" >> "$OUT/steps.txt"
  goto $((ox + cx * z / 100)) $((oy + cy * z / 100))
  click
}
open_url() { key ctrl-l; sleep 0.4; typestr "$1"; key ret; }
: > "$OUT/steps.txt"

wait_first_frame
dock_icon browser; click; sleep 3

# ---- Preact + htm TodoMVC ---- (ONLY=preact or ONLY=vue runs one part; the verdict then misses the other)
if [ "${ONLY:-}" != vue ]; then
open_url "$SITE/preact"
wait_for "todo: at input=" 180
sleep 2
shot a-preact-loaded
click_at todo input
sleep 1
n=$(count "todo: n=")
typestr "leite"; key ret
wait_more "todo: n=1 left=1" 0 60
typestr "pao"; key ret
wait_for "todo: n=2 left=2" 60
typestr "cafe"; key ret
wait_for "todo: n=3 left=3 filter=all shown=leite|pao|cafe" 60
wait_for "items=3" 60
sleep 2
shot b-preact-three
click_at todo toggle2
wait_for "todo: n=3 left=2 filter=all shown=leite|pao+|cafe" 60
sleep 2
shot c-preact-toggled
click_at todo active
wait_for "filter=active shown=leite|cafe" 60
wait_for "items=2" 30
sleep 2
shot d-preact-active
click_at todo all
wait_for "items=3" 60
sleep 2
click_at todo destroy1
wait_for "todo: n=2 left=1 filter=all shown=pao+|cafe" 60
sleep 4   # the store is written at most every 3 s
# A reload: the list comes back from localStorage.
key ctrl-r
wait_more "todo: n=2 left=1" 1 120
sleep 2
shot e-preact-reloaded
fi

# ---- Vue 3 ----
if [ "${ONLY:-}" != preact ]; then
open_url "$SITE/vue"
wait_for "vue: at inc=" 240
sleep 2
shot f-vue-loaded
click_at vue inc
wait_for "vue: count count=1" 60
click_at vue inc
wait_for "vue: count count=2" 60
click_at vue name
sleep 1
typestr "Ana"
wait_for 'vue: name count=2 name="Ana"' 60
click_at vue fruit
sleep 1
typestr "uva"; key ret
wait_for "vue: fruits .*fruits=banana|caju|uva dom=3" 60
wait_more "vue: at inc=" 1 30
sleep 1
click_at vue show
wait_for "show=true extra=true" 60
wait_more "vue: at inc=" 2 30
sleep 1
click_at vue select
wait_for "vue: color .*color=verde" 60
click_at vue msg
sleep 1
typestr "oi"; key ret
wait_for 'vue: sent .*sent="oi"' 60
sleep 3
shot g-vue-done
fi

# ---- a Web Worker ----
if [ "${ONLY:-}" = "" ] || [ "${ONLY:-}" = worker ]; then
open_url "$SITE/worker"
wait_for "worker: page got" 180
sleep 2
shot h-worker
fi
flush_snaps

{
  grep -a "\[jsrun:" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' > "$OUT/js-out.txt"
  ok=1
  need() { if grep -aqE -- "$1" "$OUT/js-out.txt"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needlog() { if grep -aqE -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  need "^todo: preact function hooks function htm function" "Preact, hooks and htm loaded"
  need "^todo: n=0 left=0 filter=all shown= dom=0 input=\"\" focus=in" "first render, input.focus() from an effect"
  need "^todo: at input=[0-9]+,[0-9]+" "getBoundingClientRect after the layout (ResizeObserver)"
  need "^todo: n=0 left=0 filter=all shown= dom=0 input=\"leite\"" "typed text reaches the app (input events, value)"
  need "^todo: enter value=\"leite\"" "keydown Enter"
  need "^todo: n=1 left=1 filter=all shown=leite dom=1 input=\"\"" "Enter adds the item and the field is cleared"
  need "^todo: n=3 left=3 filter=all shown=leite\|pao\|cafe dom=3" "three items"
  need "^todo: n=3 left=2 filter=all shown=leite\|pao\+\|cafe dom=3" "the check box marks one (click, change)"
  need "^todo: n=3 left=2 filter=active shown=leite\|cafe dom=2" "the hash link filters (hashchange)"
  needlog "browser: the page's address is now $SITE/preact#/active \(pushState\)" "the address bar follows (pushState)"
  need "^todo: n=2 left=1 filter=all shown=pao\+\|cafe dom=2" "remove"
  needlog "webstorage: saved" "localStorage written to ~/.webstorage"
  if [ "$(grep -ac "todo: n=2 left=1 filter=all shown=pao+|cafe dom=2 input=\"\" focus=in" "$OUT/js-out.txt")" -ge 1 ] && \
     [ "$(grep -c "browser: .* script(s), .* handed to pid" "$OUT/serial.log")" -ge 2 ]; then
    echo "PASS the list is back after a reload (localStorage)"
  else
    echo "MISS the list is back after a reload (localStorage)"; ok=0
  fi
  need "^vue: mounted count=0 name=\"\" fruits=banana\|caju dom=2 show=false extra=false color=vermelho sent=\"\" hello=\"Digite seu nome.\"" "Vue mounted the in-DOM template"
  need "^vue: count count=2 .*countdom=2" "@click re-renders the counter"
  need "^vue: name count=2 name=\"Ana\" .*hello=\"Ola, Ana!\"" "v-model and v-if"
  need "^vue: fruits .*fruits=banana\|caju\|uva dom=3" "@keyup.enter and v-for"
  need "^vue: show .*show=true extra=true" "check box with v-model"
  need "^vue: color .*color=verde" "<select> with v-model (change)"
  need "^vue: sent .*sent=\"oi\"" "form with @submit.prevent"
  if grep -aqE "^vue: (error|warn)|^Uncaught" "$OUT/js-out.txt"; then
    echo "MISS no errors on the pages ($(grep -aE "^vue: (error|warn)|^Uncaught" "$OUT/js-out.txt" | head -2 | tr '\n' ' '))"; ok=0
  else
    echo "PASS no errors on the pages"
  fi
  if grep -aq "fetch: GET $SITE/vue?" "$OUT/serial.log"; then echo "MISS the prevented form was sent"; ok=0; else echo "PASS the prevented form was not sent"; fi
  needlog "browser: worker 1 of pid [0-9]+ is pid [0-9]+ \($SITE/assets/worker-job.js\)" "the worker runs in a process of its own"
  need '^worker: page got \{"n":20000,"count":2262,"name":"primos","fetched":true,"doc":"undefined","date":true\}' "the worker counts, fetches and answers (postMessage both ways)"
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
