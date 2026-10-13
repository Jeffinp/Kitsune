# W55: the network of the page's scripts. The page /net of tools/w24-site.py (routes in
# tools/w24net.py) runs fetch (JSON, POST, PUT, HEAD, redirect, chunked, Range, a 4 MiB stream read
# with getReader, an 8 MiB arrayBuffer, AbortController), XMLHttpRequest, cookies (Set-Cookie with
# HttpOnly, document.cookie both ways), CORS (allowed, refused, with credentials, pre-flight, no-cors)
# against the port below the page's, the ad blocker on a fetch, the small APIs, and counts the server's
# connections (keep-alive). Each check prints `net: PASS name` or `net: FAIL name ...` on the serial
# log; the verdict is in $OUT/verdict.txt. With TLS_PORT set (tools/w24-site.py PORT --tls TLS_PORT),
# it then opens https://.../net/speed, clicks "continuar mesmo assim" (self-signed certificate) and
# records the TLS download rate (two 8 MiB downloads on one kept-alive connection) in $OUT/speed.txt.
#   tools/w24-site.py 8079 --tls 8443 &
#   QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     PORT=8079 TLS_PORT=8443 tools/perf/run.sh <img> bios <out> 900 tools/perf/scen/w55-net-js.sh
source "$(dirname "$0")/../lib.sh"
PORT=${PORT:-8079}
wait_for() { # wait_for PATTERN SECONDS
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "http://203.0.113.5:$PORT/net"
key ret
wait_for "net: done" 420
sleep 2
shot a-net
if [ -n "${TLS_PORT:-}" ]; then
  key ctrl-l; sleep 0.3
  typestr "https://203.0.113.5:$TLS_PORT/net/speed"
  key ret
  wait_for "tls: certificate refused" 120
  sleep 4
  shot b-cert
  goto 608 486; click
  wait_for "net: speed done" 420
  sleep 2
  shot c-speed
fi
# The cookie file is written a few seconds after the change.
sleep 5
flush_snaps

{
  grep -a "\[jsrun:" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' > "$OUT/js-out.txt"
  ok=1
  need() { if grep -aq -- "$1" "$OUT/js-out.txt"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  needlog() { if grep -aq -- "$1" "$OUT/serial.log"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  for name in "fetch json" "set-cookie and document.cookie" "cookies sent (HttpOnly too) and document.cookie write" \
    "POST json echo with Origin and Referer" "PUT with ArrayBuffer body" "HEAD" "XHR GET json" "XHR POST form" \
    "CORS allowed (Allow-Origin \*)" "CORS refused without the header" "CORS with credentials" \
    "CORS without credentials sends no cookie" "CORS pre-flight for PUT" "no-cors is opaque" "redirect followed" \
    "chunked body" "Range request" "streaming 4 MiB with getReader" "8 MiB arrayBuffer" "abort with AbortController" \
    "blocked by the ad blocker" "small APIs" "keep-alive (connections reused)"; do
    need "^net: PASS $name" "$name"
  done
  if grep -aq "^net: FAIL" "$OUT/js-out.txt"; then echo "MISS no check failed"; ok=0; else echo "PASS no check failed"; fi
  needlog "cookies: stored theme from" "Set-Cookie stored by the fetcher"
  needlog "cors: blocked http://203.0.113.5:$((PORT - 1))/cors/none" "CORS refusal on the serial log"
  needlog "cors: pre-flight ok for PUT" "pre-flight logged"
  needlog "shields: blocked ad fetch" "the ad blocker counted the fetch"
  needlog "net: reusing connection" "connections reused"
  needlog "cookies: saved" "cookies written to ~/.cookies"
  if [ -n "${TLS_PORT:-}" ]; then
    need "^net: speed round 2 https:" "TLS download measured"
  fi
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
{
  grep -a "^net: PASS \(streaming\|8 MiB\)\|^net: speed round" "$OUT/js-out.txt"
  grep -a "jsfetch: .* done: " "$OUT/serial.log" | tr -d '\r' | tail -6
  grep -a "net: new connection\|tls: UNVERIFIED\|handshake" "$OUT/serial.log" | tr -d '\r' | tail -8
} > "$OUT/speed.txt" 2>&1
finish
