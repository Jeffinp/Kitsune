# Hardware: what the Kitsune boots on and what it does not drive yet

Everything here is measured, not assumed. Where a row says "tested" the test is a QEMU run you can
repeat; nothing below was run on a physical machine yet (`tools/hw-check.ps1` is the script for
that, see the end).

## Video

The kernel does not have a driver per graphics card. It draws into the linear framebuffer the
firmware hands over (VBE in BIOS mode, GOP in UEFI mode) and supports 32-bit BGR/RGB at up to
1920x1080. So the question for a new machine is only whether its firmware offers such a
framebuffer. `tools/gpu-matrix.sh <outdir>` boots the release image on each adapter QEMU emulates
and checks that the desktop comes up (serial log has `TSC calibrated`, no panic, screenshot not
blank):

| Adapter (QEMU)           | BIOS (VBE)       | UEFI (GOP)       |
|--------------------------|------------------|------------------|
| `-vga std`               | ok, 1280x720     | ok, 1280x800     |
| `-vga virtio` (virtio-vga) | ok, 1280x720   | ok, 1280x800     |
| `-vga vmware`            | ok, 1280x720     | ok, 1280x800     |
| `-vga qxl`               | ok, 1280x720     | ok, 1280x800     |
| `-vga cirrus`            | ok, 800x600      | not applicable   |
| `bochs-display`          | not applicable   | ok, 1280x800     |
| `ramfb`                  | not applicable   | ok, 800x600      |
| `virtio-gpu-pci` (no VGA)| not applicable   | **fails**        |

**Known limitation.** A display that offers only *Blt* (no linear framebuffer) stops the boot loader
before the kernel runs: the firmware's GOP reports a blit-only mode and the loader panics with
"Cannot access the framebuffer in a Blt-only mode". QEMU's plain `virtio-gpu-pci` does this under
UEFI; `virtio-vga` (the same device with a VGA front) works. Physical PCs and the usual
hypervisors expose a linear framebuffer, so this mostly matters for a virtio-gpu-only VM. Supporting
it means driving the virtio-gpu scanout from the kernel (the 2D command path is already verified at
boot, see `kernel/src/virtio_gpu.rs`) and booting without a firmware framebuffer.

A screen larger than 1920x1080 is refused with an on-screen message that says what was detected.

## Other devices

| Function  | Driven                                         | Not driven yet                                   |
|-----------|------------------------------------------------|--------------------------------------------------|
| Network   | virtio-net, NE2000 (ISA)                       | Intel e1000/e1000e, Realtek, Wi-Fi               |
| Storage   | IDE/ATA (PIO), used for the filesystem disk    | AHCI/SATA native, NVMe (a machine whose disk is only NVMe/AHCI has no persistent filesystem) |
| Input     | PS/2 keyboard and mouse (whether a firmware's USB legacy emulation reaches them is untested); **USB (xHCI) boot-protocol keyboards and mice**, polled, up to 4 devices on the root ports, proven in QEMU only (`docs/design/usb-xhci.md`) | USB hubs and hot-plug, touchpads (I2C-HID), report-protocol devices, EHCI/OHCI/UHCI controllers |
| Entropy   | RDSEED/RDRAND, virtio-rng, timing jitter       | -                                                |
| Clock     | CMOS RTC, SNTP when the network is up          | -                                                |

On a physical machine, the boot log (**Apps > Registro**) lists every PCI device found as
`vendor:device`, which is exactly what is needed to decide the next driver.

## Trying a physical machine

1. Windows: `.\tools\hw-check.ps1` boots the image on each emulated adapter with the host's QEMU and
   writes screenshots and a report under `hw-report\`, then builds `kitsune-uefi.img` for a USB
   stick (see [BOOT-USB.md](BOOT-USB.md)).
2. Boot the stick (UEFI, Secure Boot off) and note: does the desktop appear with the right
   colours and resolution, do keyboard and mouse work, and what does Registro list.
