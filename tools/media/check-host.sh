#!/usr/bin/env bash
# check-host.sh MEDIA_DIR [extra mediacheck args]: decode every test file of MEDIA_DIR (made by
# gen-test-media.sh) on the host with mediadec's C and pipeline (tools/media/mediacheck) and check
# the pictures' MD5 against ffmpeg's framemd5 and the PCM against ffmpeg's decode (SNR > 40 dB, same
# sample count). Prints PASS/FAIL per file; exit status 1 if any fails.
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
  "$target/release/mediacheck" "$media/$n.webm" --vmd5 "$media/$n.vmd5" --aref "$media/$n.ref.raw" "$@" || fail=1
  "$target/release/mediacheck" "$media/$n.webm" --serve-test || fail=1
done
exit $fail
