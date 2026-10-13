# Build (release) + run Kitsune in QEMU on Windows.
#
# The kernel is compiled inside WSL (cargo/Rust live there), the resulting
# bootable image is copied next to this script, and QEMU runs on Windows.
#
# Usage:
#   .\run.ps1                # build release + boot (WHPX + SDL display)
#   .\run.ps1 -NoAccel       # software CPU emulation (TCG) instead of WHPX
#   .\run.ps1 -Gl            # SDL with gl=on: GPU-uploads the framebuffer for a
#                            #   faster blit (needs a QEMU build with OpenGL)
#   .\run.ps1 -SoftwareGfx   # keep QEMU's default display (escape hatch if the
#                            #   SDL/GL path misbehaves on this machine)
#   .\run.ps1 -SkipBuild     # skip cargo build, just boot the existing image
#   .\run.ps1 -Native       # build with the Rust installed on Windows itself (no WSL).
#                            #   Used automatically when `cargo` is on the Windows PATH.
#   .\run.ps1 -Wsl          # force the WSL build even if Windows has cargo
#   .\run.ps1 -NoRng        # do NOT add the virtio-rng entropy device (tests the
#                            #   timing-jitter path; by default `-device virtio-rng-pci` is
#                            #   added when this QEMU build has it, so the kernel logs
#                            #   "RNG: strong" even under WHPX, where RDRAND is hidden)
#   .\run.ps1 -NoAudio       # no sound card (by default an AC'97 plays through Windows'
#                            #   DirectSound: `-audiodev dsound,id=snd0 -device AC97,audiodev=snd0`)
#   .\run.ps1 -Usb           # build the UEFI image + copy it out for USB flashing
#                            #   (real hardware boot; does NOT launch QEMU)
param(
    [switch]$NoAccel,
    [switch]$SkipBuild,
    [switch]$Gl,
    [switch]$SoftwareGfx,
    [switch]$Usb,
    [switch]$Native,
    [switch]$Wsl,
    [switch]$Doom,
    [switch]$NoRng,
    [switch]$NoAudio
)

$ErrorActionPreference = 'Stop'
$qemu = 'C:\Program Files\qemu\qemu-system-x86_64.exe'
$img = Join-Path $PSScriptRoot 'kitsune-bios.img'

# Build the project's `os` package (release) inside WSL. Returns nothing; throws
# on failure. Factored out so both the QEMU and USB paths share it.
function Build-Kitsune {
    # Native Windows build when asked for, or when cargo is on the Windows PATH
    # (and -Wsl did not force WSL). DOOM needs wasi-sdk, which only the WSL path supports.
    $useNative = $Native -or ((Get-Command cargo -ErrorAction SilentlyContinue) -and -not $Wsl -and -not $Doom)
    if ($useNative) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            throw "cargo nao encontrado no Windows. Instale o Rust: https://rustup.rs (rustup-init.exe)"
        }
        # user-apps/jsrun compiles QuickJS with clang (build.rs reads $env:CLANG). The LLVM installer
        # does not put clang on PATH by default, so fall back to its usual install dir.
        if (-not $env:CLANG -and -not (Get-Command clang -ErrorAction SilentlyContinue)) {
            $llvmClang = 'C:\Program Files\LLVM\bin\clang.exe'
            if (-not (Test-Path $llvmClang)) {
                throw "clang nao encontrado (necessario para o jsrun). Instale: winget install LLVM.LLVM"
            }
            $env:CLANG = $llvmClang
        }
        Write-Host "Compilando Kitsune (release) no Windows ..." -ForegroundColor Cyan
        Push-Location $PSScriptRoot
        try {
            cargo build --package os --release
            if ($LASTEXITCODE -ne 0) { throw "cargo build falhou (exit $LASTEXITCODE)" }
        } finally { Pop-Location }
        return
    }
    $drive = $PSScriptRoot.Substring(0, 1).ToLower()
    $rest = ($PSScriptRoot.Substring(2)) -replace '\\', '/'
    $wslDir = "/mnt/$drive$rest"
    # -Doom builds the DOOM app variant (C→wasm32-wasi + embedded IWAD). Needs
    # wasi-sdk at ~/wasi-sdk and the IWAD at wasm-apps/doom/doom1.wad in WSL.
    $envPrefix = if ($Doom) { 'DOOM=1 WASI_SDK_PATH=$HOME/wasi-sdk ' } else { '' }
    Write-Host "Compilando Kitsune (release) no WSL em $wslDir ..." -ForegroundColor Cyan
    wsl -e bash -lc "cd '$wslDir' && ${envPrefix}cargo build --package os --release"
    if ($LASTEXITCODE -ne 0) { throw "cargo build falhou (exit $LASTEXITCODE)" }
}

if ($Usb) {
    # Real-hardware boot: build, then copy the UEFI image to the project root so
    # it can be flashed RAW to a USB stick. No QEMU involved.
    Build-Kitsune
    $built = Get-ChildItem -Path (Join-Path $PSScriptRoot 'target\release\build') `
        -Recurse -Filter 'kitsune-uefi.img' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $built) { throw "kitsune-uefi.img nao encontrada apos o build" }
    $usbImg = Join-Path $PSScriptRoot 'kitsune-uefi.img'
    Copy-Item $built.FullName $usbImg -Force
    Write-Host ""
    Write-Host "Imagem UEFI pronta: $usbImg" -ForegroundColor Green
    Write-Host "Grave-a CRUA num pendrive (apaga o pendrive inteiro!):" -ForegroundColor Yellow
    Write-Host "  - Rufus: selecione a imagem, modo 'DD Image'; ou" -ForegroundColor Gray
    Write-Host "  - balenaEtcher: Flash from file -> escolha o pendrive." -ForegroundColor Gray
    Write-Host "No PC alvo: firmware UEFI, Secure Boot DESLIGADO, dê boot pelo pendrive." -ForegroundColor Gray
    Write-Host "Detalhes e limitacoes: docs/BOOT-USB.md" -ForegroundColor Gray
    exit 0
}

if (-not (Test-Path $qemu)) { throw "QEMU nao encontrado em $qemu" }

if (-not $SkipBuild) {
    Build-Kitsune

    # Copy the freshest generated image to the project root.
    $built = Get-ChildItem -Path (Join-Path $PSScriptRoot 'target\release\build') `
        -Recurse -Filter 'kitsune-bios.img' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $built) { throw "imagem kitsune-bios.img nao encontrada apos o build" }
    Copy-Item $built.FullName $img -Force
    Write-Host "Imagem pronta: $img" -ForegroundColor Green
}

if (-not (Test-Path $img)) { throw "Imagem nao existe: $img (rode sem -SkipBuild)" }

# Persistent filesystem disk (secondary IDE master). Created blank on first run;
# the kernel formats it on first boot and persists files there across reboots.
$fsImg = Join-Path $PSScriptRoot 'kitsune-fs.img'
if (-not (Test-Path $fsImg)) {
    # 64 MiB sparse-ish file (OJFS v3 needs >= 1 MiB). An older 64 KiB disk still
    # boots: the kernel stays on OJFS v2 ("disk too small for OJFS v3" on the serial).
    $fsFile = [System.IO.File]::Create($fsImg)
    $fsFile.SetLength(64MB)
    $fsFile.Close()
    Write-Host "Disco de arquivos criado: $fsImg" -ForegroundColor Green
}

$pcap = Join-Path $PSScriptRoot 'kitsune-net.pcap'
# DOOM grows its wasm guest memory well past the default; give it more RAM.
$mem = if ($Doom) { '512M' } else { '256M' }
$qargs = @('-m', $mem, '-drive', "format=raw,file=$img",
    '-drive', "format=raw,file=$fsImg,if=ide,index=2",
    '-netdev', 'user,id=n0',
    '-device', 'ne2k_isa,netdev=n0,mac=52:54:00:12:34:56',
    '-object', "filter-dump,id=dump,netdev=n0,file=$pcap")
if (-not $NoAccel) { $qargs = @('-accel', 'whpx') + $qargs }

# Host entropy for the kernel's RNG (virtio-rng-pci). WHPX hides RDRAND/RDSEED from the guest,
# so without this device the kernel falls back to timing jitter ("RNG: pool seeded from timing
# jitter"). Skipped if this QEMU build lacks the device or -NoRng was given.
if (-not $NoRng) {
    $devs = (& $qemu -device help 2>&1 | Out-String)
    if ($devs -match 'name "virtio-rng-pci"') {
        $qargs += @('-device', 'virtio-rng-pci')
    } else {
        Write-Host "Este QEMU nao tem virtio-rng-pci; o RNG usara jitter de temporizacao." -ForegroundColor Yellow
    }
}

# Sound: an AC'97 card played through DirectSound (docs/design/audio.md). The kernel logs
# "audio: AC'97 up" and the Terminal's `tone 440 1000`, `play file.wav` and `volume` use it; without the
# card it logs "audio: no device" and boots the same. Skipped if this QEMU build lacks AC97 or dsound,
# or with -NoAudio.
if (-not $NoAudio) {
    $devs = (& $qemu -device help 2>&1 | Out-String)
    $drivers = (& $qemu -audiodev help 2>&1 | Out-String)
    if ($devs -match 'name "AC97"' -and $drivers -match '\bdsound\b') {
        $qargs += @('-audiodev', 'dsound,id=snd0', '-device', 'AC97,audiodev=snd0')
        Write-Host "Som: AC'97 via DirectSound" -ForegroundColor DarkGray
    } else {
        Write-Host "Este QEMU nao tem AC97 ou dsound; o sistema sobe sem som." -ForegroundColor Yellow
    }
}

# Display. The kernel paints a full 1920x1080x4 (~8 MiB) linear framebuffer each
# dirty frame; QEMU's default Windows display (GTK/Cairo) re-uploads that surface
# in software, which dominates the per-frame cost. SDL uses a faster host
# renderer; `-Gl` adds gl=on to offload the VRAM->window upload to the GPU.
#
# Official Windows QEMU builds differ in which backends they ship -- naming an
# absent one ("-display sdl" on a GTK-only build) makes QEMU exit immediately
# (window flashes open then closes). So probe `-display help` and pick the best
# backend actually present, preferring SDL; if neither is found we omit the flag
# and let QEMU use its default. `-vga std` is pinned because the bootloader takes
# its framebuffer from VBE -- virtio GPUs expose none without a guest driver and
# would boot to a black screen.
if (-not $SoftwareGfx) {
    # GTK is the only reliable backend on the official Windows QEMU builds: SDL is
    # listed by `-display help` but exits at init (window flashes and closes), and
    # gl=on is buggy on Windows for both backends -- so we do NOT enable GL by
    # default. `-Gl` lets you try gtk,gl=on at your own risk.
    #   refs: gitlab.com/qemu-project/qemu issues #2200, #1530; quickemu #967
    $avail = (& $qemu -display help 2>&1 | Out-String)
    $backend = @('gtk', 'sdl') | Where-Object { $avail -match "\b$_\b" } | Select-Object -First 1
    if ($backend) {
        $display = if ($Gl) { "$backend,gl=on" } else { $backend }
        $qargs += @('-vga', 'std', '-display', $display)
        Write-Host "Display: $display" -ForegroundColor DarkGray
    } else {
        Write-Host "Nenhum backend GTK/SDL; usando display padrao do QEMU." -ForegroundColor Yellow
    }
}

Write-Host "Booting Kitsune..." -ForegroundColor Cyan
& $qemu @qargs
