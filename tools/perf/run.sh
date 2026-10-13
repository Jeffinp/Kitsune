#!/usr/bin/env bash
# run.sh <img> <bios|uefi> <outdir> <maxsecs> [scenario-script]
#
# Headless QEMU run of a built image (`cargo build --release -p os [--features
# perf-trace]`; images are under target/release/build/os/*/out/kitsune-*.img).
# Serial goes to <outdir>/serial.log. The optional scenario script
# (tools/perf/scen/*.sh; gets $OUT and $MODE) drives the QEMU monitor (mouse/keys)
# in the background and touches $OUT/done when finished; then we screendump to
# <outdir>/screen.png and quit.
# Env: QEMU_MEM (default 128M, 512M for uefi), QEMU_EXTRA (e.g. "-icount shift=0"),
# FS_SIZE (filesystem disk size, default 64M; 64K gives the old OJFS v2-only disk).
# FS_IMG (a filesystem disk image to boot with instead of a blank one, to test persistence).
# QEMU_RNG (virtio|none, default virtio): add `-device virtio-rng-pci` (RNG: strong) or leave it out
# to force the timing-jitter path.
# QEMU_NIC (default ne2k_isa; virtio-net-pci for the faster virtio driver).
# QEMU_NETDEV (default "user,id=n0"; the id must stay n0), e.g. to give the guest a fake
# public-looking network (see tools/nettest-server.py):
#   QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3"
set -u
img=$1; mode=$2; out=$3; secs=$4; scen=${5:-}
rm -rf "$out"; mkdir -p "$out"
# Unix socket paths are limited to 107 bytes, so keep the monitor socket short.
export MON_SOCK=$(mktemp -u /tmp/kit-mon.XXXXXX)
echo "$MON_SOCK" > "$out/mon.path"
if [ -n "${FS_IMG:-}" ]; then cp "$FS_IMG" "$out/fs.img"; else truncate -s "${FS_SIZE:-64M}" "$out/fs.img"; fi  # sparse 64 MiB (OJFS v3 needs >= 1 MiB)
# Entropy device: QEMU_RNG=virtio (default) adds `-device virtio-rng-pci` so the kernel
# has a host entropy source (RNG: strong); QEMU_RNG=none leaves it out to exercise the
# timing-jitter path (the default CPU, qemu64, has no RDRAND/RDSEED either; `-- -cpu max`
# adds them). Skipped silently on a QEMU build without the device.
rng_dev=()
case "${QEMU_RNG:-virtio}" in
  virtio) if qemu-system-x86_64 -device help 2>&1 | grep -q 'name "virtio-rng-pci"'; then
            rng_dev=(-device virtio-rng-pci)
          fi ;;
  none)   ;;
  *) echo "unknown QEMU_RNG '${QEMU_RNG}' (use virtio or none)" >&2; exit 2 ;;
esac
mem=${QEMU_MEM:-128M}; [ "$mode" = uefi ] && mem=${QEMU_MEM:-512M}
args=(-m "$mem" -display none -no-reboot -no-shutdown
  -serial "file:$out/serial.log"
  -monitor "unix:$MON_SOCK,server,nowait"
  -drive "format=raw,file=$img,file.locking=off"
  -drive "format=raw,file=$out/fs.img,if=ide,index=2"
  -netdev "${QEMU_NETDEV:-user,id=n0}" -device "${QEMU_NIC:-ne2k_isa}",netdev=n0,mac=52:54:00:12:34:56 "${rng_dev[@]}")
if [ "$mode" = uefi ]; then
  cp /usr/share/OVMF/OVMF_VARS_4M.fd "$out/ovmf_vars.fd"
  args=(-drive "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd"
        -drive "if=pflash,format=raw,file=$out/ovmf_vars.fd" "${args[@]}")
fi
qemu-system-x86_64 "${args[@]}" ${QEMU_EXTRA:-} &
qpid=$!
trap 'kill $qpid 2>/dev/null; rm -f "$MON_SOCK"' EXIT
if [ -n "$scen" ]; then (OUT=$out MODE=$mode bash "$scen") & fi
start=$(date +%s)
while [ ! -f "$out/done" ] && [ $(( $(date +%s) - start )) -lt "$secs" ]; do sleep 0.5; done
if [ -z "$scen" ]; then sleep "$secs"; fi
echo "screendump $out/screen.ppm" | socat - "UNIX-CONNECT:$MON_SOCK" >/dev/null
sleep 1
convert "$out/screen.ppm" "$out/screen.png" 2>/dev/null
rm -f "$out/screen.ppm"
echo quit | socat - "UNIX-CONNECT:$MON_SOCK" >/dev/null 2>&1
wait $qpid 2>/dev/null
