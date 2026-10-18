# W65 (Windows): a system service that dies is brought back. Boots the image, types `crash fetcher` in the Terminal
# and reads the serial log: the thread dies, the supervisor frees its stack and restarts it after a delay; a few
# deaths in a row exhaust the budget and the service is left stopped. See docs/design/supervisao.md.
#
#   .\tools\qemu-supervise.ps1                      # newest image of target\release\build\os
#   .\tools\qemu-supervise.ps1 -Image kitsune-bios.img -Accel whpx
#
# Result: .shots\w65\w65-result.txt (PASS/FAIL per check), serial.log and screenshots. Exit code 1 on any failure.
param(
    [string]$Image,
    [string]$Accel = 'tcg',
    [string]$Service = 'fetcher',
    [int]$Port = 4456,
    [int]$BootTimeout = 150
)
$ErrorActionPreference = 'Stop'
$qemu = 'C:\Program Files\qemu\qemu-system-x86_64.exe'
$root = Split-Path $PSScriptRoot
$out = Join-Path $root '.shots\w65'
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $out | Out-Null
if (-not $Image) {
    $Image = Get-ChildItem (Join-Path $root 'target\release\build\os') -Recurse -Filter 'kitsune-bios.img' |
        Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $Image -or -not (Test-Path $Image)) { throw 'imagem kitsune-bios.img nao encontrada (cargo build --release -p os)' }

$serial = Join-Path $out 'serial.log'
$fs = Join-Path $out 'fs.img'
$f = [IO.File]::Create($fs); $f.SetLength(64MB); $f.Close()
$qargs = @('-accel', $Accel, '-m', '256M', '-display', 'none', '-no-reboot',
    '-serial', "file:$serial", '-qmp', "tcp:127.0.0.1:${Port},server,nowait",
    '-drive', "format=raw,file=$Image", '-drive', "format=raw,file=$fs,if=ide,index=2",
    # the fetcher only exists when there is a NIC (without one the browser runs offline)
    '-netdev', 'user,id=n0', '-device', 'ne2k_isa,netdev=n0,mac=52:54:00:12:34:56')
$proc = Start-Process -FilePath $qemu -ArgumentList $qargs -PassThru -WindowStyle Hidden
$results = New-Object System.Collections.Generic.List[string]
$failed = $false
function Check([bool]$ok, [string]$what) {
    $line = $(if ($ok) { 'PASS ' } else { 'FAIL ' }) + $what
    $script:results.Add($line); Write-Host $line
    if (-not $ok) { $script:failed = $true }
}
function Log() { if (Test-Path $serial) { Get-Content $serial -Raw } else { '' } }
function WaitLog([string]$pattern, [int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) { if ((Log) -match $pattern) { return $true }; Start-Sleep -Milliseconds 500 }
    return $false
}
try {
    if (-not (WaitLog 'first desktop frame' $BootTimeout)) { throw 'o desktop nao subiu a tempo' }
    Start-Sleep -Seconds 4
    $c = New-Object System.Net.Sockets.TcpClient
    $c.Connect('127.0.0.1', $Port)
    $s = $c.GetStream()
    $r = New-Object IO.StreamReader($s)
    $w = New-Object IO.StreamWriter($s); $w.AutoFlush = $true
    [void]$r.ReadLine()
    $w.WriteLine('{"execute":"qmp_capabilities"}'); [void]$r.ReadLine()
    function Qmp([string]$json) {
        $w.WriteLine($json)
        while ($true) { $l = $r.ReadLine(); if ($l -notmatch '"event"') { return $l } }
    }
    function Key([string]$code, [bool]$shift = $false) {
        $codes = if ($shift) { @('shift', $code) } else { @($code) }
        $down = ($codes | ForEach-Object { '{"type":"key","data":{"down":true,"key":{"type":"qcode","data":"' + $_ + '"}}}' }) -join ','
        $rev = [string[]]($codes[($codes.Count - 1)..0])
        $up = ($rev | ForEach-Object { '{"type":"key","data":{"down":false,"key":{"type":"qcode","data":"' + $_ + '"}}}' }) -join ','
        [void](Qmp ('{"execute":"input-send-event","arguments":{"events":[' + $down + ']}}'))
        Start-Sleep -Milliseconds 60
        [void](Qmp ('{"execute":"input-send-event","arguments":{"events":[' + $up + ']}}'))
        Start-Sleep -Milliseconds 80
    }
    function Enter-Text([string]$text) {
        foreach ($ch in $text.ToCharArray()) {
            if ($ch -eq ' ') { Key 'spc' } else { Key ([string]$ch) }
        }
    }
    function Crash() {
        Enter-Text "crash $Service"; Key 'ret'
    }
    function Shot([string]$name) {
        $ppm = (Join-Path $out "$name.ppm") -replace '\\', '\\\\'
        [void](Qmp ('{"execute":"screendump","arguments":{"filename":"' + $ppm + '"}}'))
        Start-Sleep -Milliseconds 1500
    }

    # the Terminal that the desktop opens at boot has the focus; click it once to be sure
    [void](Qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"rel","data":{"axis":"x","value":-200}},{"type":"rel","data":{"axis":"y","value":-100}}]}}')
    Start-Sleep -Milliseconds 300
    [void](Qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}')
    Start-Sleep -Milliseconds 80
    [void](Qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}')
    Start-Sleep -Seconds 1

    $name = [regex]::Escape($Service)
    Crash
    Shot 'typed'
    Check (WaitLog "thread '$name' died: panicked at .*supervisor.rs:\d+:\d+:\s+fault injected by" 60) "crash $Service made the thread panic and die"
    Check (WaitLog "supervisor: '$name' died; stack freed, restart in \d+ ms \(attempt 1 of 3\)" 30) 'the supervisor freed the stack and scheduled attempt 1'
    Check (WaitLog "supervisor: '$name' restarted in slot \d+" 60) 'the service was restarted in its slot'
    Shot 'after-first-restart'

    # the restarted thread is a normal one: it can be crashed again, and the delays grow
    Start-Sleep -Seconds 2
    Crash
    Check (WaitLog "attempt 2 of 3" 60) 'a second death waits longer (attempt 2 of 3)'
    Check (([regex]::Matches((Log), "supervisor: '$name' restarted")).Count -ge 2 -or (WaitLog "supervisor: '$name' restarted in slot \d+[\s\S]*supervisor: '$name' restarted in slot" 60)) 'and the service came back again'
    Start-Sleep -Seconds 2
    Crash
    Check (WaitLog "attempt 3 of 3" 90) 'a third death in the window is attempt 3 of 3'
    Check (WaitLog "supervisor: '$name' restarted in slot \d+[\s\S]*supervisor: '$name' restarted in slot \d+[\s\S]*supervisor: '$name' restarted in slot" 120) 'and it came back a third time'
    Start-Sleep -Seconds 2
    Crash
    Check (WaitLog "supervisor: '$name' died \d+ times in 60 s: not restarted" 60) 'the fourth death in the window exhausts the budget: not restarted'
    Shot 'gave-up'
    Check ((Log) -notmatch 'panic: ') 'no fatal panic in the serial log'
    [void](Qmp '{"execute":"quit"}')
    $c.Close()
}
catch {
    Check $false ("script error: " + $_.Exception.Message)
}
finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    $results | Set-Content (Join-Path $out 'w65-result.txt')
}
Write-Host "Resultado em $out"
if ($failed) { exit 1 }
