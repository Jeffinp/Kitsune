#!/usr/bin/env bash
# check-host.sh MEDIA_DIR [extra mediacheck args]: decode every test file of MEDIA_DIR (made by
# gen-test-media.sh) on the host with mediadec's C and pipeline (tools/media/mediacheck) and check
# the pictures' MD5 against ffmpeg's framemd5 and the PCM against ffmpeg's decode (SNR > 40 dB, same
# sample count), then the message server in memory (--serve-test). Prints PASS/FAIL per check; exit
# status 1 if any fails.
#
# Also leaves, for the QEMU scenario w57-mediadec.sh, NAME.pcm.md5: the MD5 of the PCM decoded here.
# The Opus decoder is fixed point (integers only), so mediadec in the guest must give the same bytes.
# Also makes the hostile inputs of the scenario: cut.webm (v360 cut in the middle of a cluster) and
# bad.webm (v180 with bytes of its clusters overwritten).
# MEDIACHECK_TARGET picks the cargo target directory (default target/mediacheck).
set -u
media=$1
shift
here=$(cd "$(dirname "$0")" && pwd)
target=${MEDIACHECK_TARGET:-$here/../../target/mediacheck}
cargo build -q --release --manifest-path "$here/mediacheck/Cargo.toml" --target-dir "$target" || exit 2
fail=0
for n in v180 v360 v720; do
  [ -f "$media/$n.webm" ] || continue
  echo "== $n"
  "$target/release/mediacheck" "$media/$n.webm" --vmd5 "$media/$n.vmd5" --aref "$media/$n.ref.raw" \
    --pcm "$media/$n.pcm" "$@" || fail=1
  md5sum < "$media/$n.pcm" | cut -d' ' -f1 > "$media/$n.pcm.md5"
  rm -f "$media/$n.pcm"
  "$target/release/mediacheck" "$media/$n.webm" --serve-test || fail=1
done
if [ -f "$media/v360.webm" ] && [ -f "$media/v180.webm" ]; then
  head -c 60000 "$media/v360.webm" > "$media/cut.webm"
  python3 -I -c '
import sys
d = bytearray(open(sys.argv[1], "rb").read())
for i in range(20000, len(d) - 100, 997):
    d[i] = (d[i] * 31 + 7) & 255
open(sys.argv[2], "wb").write(d)' "$media/v180.webm" "$media/bad.webm"
  echo "== bad.webm (corrupted clusters: must not crash)"
  "$target/release/mediacheck" "$media/bad.webm" > /dev/null 2>&1
  [ $? -le 1 ] && echo "survived" || { echo "CRASHED"; fail=1; }
fi
exit $fail
