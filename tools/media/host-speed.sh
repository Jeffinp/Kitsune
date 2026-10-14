#!/usr/bin/env bash
# host-speed.sh DIR: how fast mediadec's VP9 decoder (the same C, tools/media/mediacheck) decodes and
# converts on this machine, natively, at the sizes and rates YouTube serves (360p to 1080p, 30 and
# 60 fps), for the arithmetic of "does 60 fps fit" (docs/design/tls-browser.md §18).
#
# The clips are made once in DIR with ffmpeg (libvpx-vp9 at YouTube-like bit rates, 2 s keyframes):
# "easy" is testsrc2 (flat colours, little motion: the test pattern of the other scenarios), "hard" a
# Mandelbrot zoom with grain (detail everywhere, all of it moving: closer to camera footage). Each
# line: decode fps (container + VP9, one thread), the conversion to RGBA at the picture's own size
# (`--rgb`, what the decoder does for a box at least that big), and the frame budget it leaves at the
# clip's rate. The built-in clip of the speed test (`small.webm`, 320x180) is measured too: its
# pixels a second against a real size's say how far the browser's first guess is from the truth.
set -eu
dir=$1
mkdir -p "$dir"
here=$(cd "$(dirname "$0")" && pwd)
target=${MEDIACHECK_TARGET:-$here/../../target/mediacheck}
cargo build -q --release --manifest-path "$here/mediacheck/Cargo.toml" --target-dir "$target"
mc=$target/release/mediacheck
make() { # make NAME SRC W H FPS KBPS
  local f=$dir/$1.webm
  [ -f "$f" ] && return
  local src
  if [ "$2" = easy ]; then src="testsrc2=size=$3x$4:rate=$5"
  else src="mandelbrot=size=$3x$4:rate=$5,noise=alls=10:allf=t"; fi
  ffmpeg -v error -y -f lavfi -i "$src" -t 10 -pix_fmt yuv420p -c:v libvpx-vp9 -b:v "$6k" -g $(( $5 * 2 )) \
    -deadline good -cpu-used 4 -row-mt 1 -an "$f"
}
printf '%-22s %10s %12s %12s %14s\n' clip decode_fps ms_decode ms_rgba ms_left_of_frame
run() { # run NAME W H FPS
  local out fps rgba
  out=$("$mc" "$dir/$1.webm" --rgb "$2x$3")
  fps=$(sed -n 's/^video: .* (\([0-9]*\) fps)/\1/p' <<< "$out")
  rgba=$(sed -n 's/^rgba: \([0-9.]*\) ms.*/\1/p' <<< "$out")
  python3 -c "
f=$fps; r=$rgba; rate=$4
dec=1000/f
print('%-22s %10d %12.2f %12.2f %14.2f' % ('$1', f, dec, r, 1000/rate - dec - r))"
}
for spec in "easy 640 360 30 700" "hard 640 360 30 700" "easy 640 360 60 1000" "hard 640 360 60 1000" \
            "hard 854 480 30 1200" "hard 854 480 60 1800" "easy 1280 720 30 2000" "hard 1280 720 30 2000" \
            "easy 1280 720 60 3000" "hard 1280 720 60 3000" "hard 1920 1080 60 5000"; do
  set -- $spec
  name="$1-$3p$4"
  make "$name" "$1" "$2" "$3" "$4" "$5"
  run "$name" "$2" "$3" "$4"
done
small=$here/../../kitsune_core/src/format/webm/testdata/small.webm
if [ -f "$small" ]; then
  out=$("$mc" "$small")
  echo "built-in clip (320x180): $(sed -n 's/^video: //p' <<< "$out")"
fi
