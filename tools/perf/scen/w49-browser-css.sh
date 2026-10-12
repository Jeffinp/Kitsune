# W49: a page whose styles come from a separate sheet: <link rel=stylesheet>, flex, grid, float, @media, var().
#   tools/w24-site.py &
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
#     tools/perf/run.sh <img> bios <out> 200 tools/perf/scen/w49-browser-css.sh
source "$(dirname "$0")/../lib.sh"
wait_first_frame
dock_icon browser; click; sleep 3
key ctrl-l; sleep 0.3
typestr "http://203.0.113.5:8079/modern"
key ret
sleep 9
shot a-modern
key pgdn; sleep 0.8
shot b-modern-2
finish
