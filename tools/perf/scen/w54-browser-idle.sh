# W54: CPU of the browser. A page open and then left alone (the desktop must go idle: zero frames per
# second in the trace), then scrolling. PAGE picks the page of tools/w24-site.py (modern, bigcss, js).
#   tools/w24-site.py &
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     tools/perf/run.sh <img-with-perf-trace> bios <out> 120 tools/perf/scen/w54-browser-idle.sh
#   python3 tools/perf/idle_tail.py <out> 10
source "$(dirname "$0")/../lib.sh"
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "http://203.0.113.5:8079/${PAGE:-modern}"
key ret
sleep 12
shot a-loaded
sleep 25
shot b-idle
for i in 1 2 3 4 5 6; do key pgdn; sleep 0.5; done
sleep 10
shot c-after-scroll
finish
