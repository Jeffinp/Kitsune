# W58 (real files): `<video>` with WebM files from the internet, on the page /real of tools/w24-site.py
# (`/real?src=URL`: the video, its events in the serial log as `media-test: ...`, played to the end,
# seeked to 1 s and played to the end again). PAGES lists the page addresses (space separated); a
# `src` of `/relay?u=URL` is the same file downloaded once by the test server (for a sandbox whose
# proxy re-signs https: the guest cannot verify another site's certificate). WAIT is the most
# seconds per page. The verdict ($OUT/verdict.txt) lists, per page, what happened.
#   tools/w24-site.py 8179 &
#   PAGES="http://203.0.113.5:8179/real?src=/relay?u=https://test-videos.co.uk/vids/bigbuckbunny/webm/vp9/360/Big_Buck_Bunny_360_10s_1MB.webm" \
#   QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     tools/perf/run.sh IMG bios OUT 900 tools/perf/scen/w58-real.sh
source "$(dirname "$0")/../lib.sh"
WAIT=${WAIT:-150}
count() { grep -ac -- "$1" "$OUT/serial.log" 2>/dev/null || true; }
wait_more() { # wait_more PATTERN BEFORE SECONDS: until the pattern has more lines than BEFORE
  local i
  for ((i = 0; i < $3 * 2; i++)); do [ "$(count "$1")" -gt "$2" ] && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
dock_icon browser; click; sleep 3
n=0
for u in $PAGES; do
  n=$((n + 1))
  d0=$(count "media-test: DONE\|media-test: error")
  key ctrl-l; sleep 0.3
  typestr "$u"
  key ret
  wait_more "media-test: playing" "$(count "media-test: playing")" 120 && { sleep 2; shot "p$n-playing"; }
  wait_more "media-test: DONE\|media-test: error" "$d0" "$WAIT"
  sleep 1
  shot "p$n-end"
done
{
  grep -a "media-test: \(metadata\|size\|playing\|first end\|DONE\|error\)\|media: element\|browser: player\|fetch: .*webm\|jsfetch: .*done" \
    "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\] //' | grep -v "media: element [0-9]* t=" | head -80
  if [ "$(count "media-test: DONE")" -gt 0 ]; then echo "VERDICT: PASS (a file played to the end)"; else echo "VERDICT: FAIL"; fi
} > "$OUT/verdict.txt" 2>&1
finish
