# W64 (Windows): prova que teclado e mouse USB (xHCI) chegam ao desktop. Sobe o QEMU sem PS/2
# (-machine pc,i8042=off) com qemu-xhci + usb-kbd + usb-mouse, injeta teclas e mouse pelo QMP e compara
# capturas de tela. Equivale a tools/perf/scen/w64-usb-hid.sh, que usa socat e ImageMagick (Linux).
#
#   .\tools\qemu-usb-hid.ps1                      # usa a imagem mais nova de target\release\build\os
#   .\tools\qemu-usb-hid.ps1 -Image kitsune-bios.img -Accel whpx
#
# Resultado: .shots\w64\w64-result.txt (PASS/FAIL por verificação), capturas .png e serial.log. Sai com 1 se algo falhar.
param(
    [string]$Image,
    [string]$Accel = 'tcg',
    [int]$Port = 4455,
    [int]$BootTimeout = 150
)
$ErrorActionPreference = 'Stop'
$qemu = 'C:\Program Files\qemu\qemu-system-x86_64.exe'
$root = Split-Path $PSScriptRoot
$out = Join-Path $root '.shots\w64'
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $out | Out-Null

if (-not $Image) {
    $Image = Get-ChildItem (Join-Path $root 'target\release\build\os') -Recurse -Filter 'kitsune-bios.img' |
        Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $Image -or -not (Test-Path $Image)) { throw 'imagem kitsune-bios.img nao encontrada (cargo build --release -p os)' }

Add-Type -TypeDefinition @'
using System;
using System.IO;
public static class Shot {
    // Reads a binary PPM (P6) and returns the width, height and RGB bytes.
    static byte[] Read(string path, out int w, out int h) {
        byte[] b = File.ReadAllBytes(path);
        int pos = 0; int[] v = new int[3]; int n = 0;
        if (b[0] != 'P' || b[1] != '6') throw new Exception("not a P6 ppm");
        pos = 2;
        while (n < 3) {
            while (char.IsWhiteSpace((char)b[pos])) pos++;
            if (b[pos] == '#') { while (b[pos] != '\n') pos++; continue; }
            int x = 0;
            while (pos < b.Length && char.IsDigit((char)b[pos])) { x = x * 10 + (b[pos] - '0'); pos++; }
            v[n++] = x;
        }
        pos++;
        w = v[0]; h = v[1];
        byte[] rgb = new byte[w * h * 3];
        Array.Copy(b, pos, rgb, 0, rgb.Length);
        return rgb;
    }
    // Number of pixels that differ between two captures.
    public static int Diff(string a, string b) {
        int w1, h1, w2, h2;
        byte[] x = Read(a, out w1, out h1), y = Read(b, out w2, out h2);
        if (w1 != w2 || h1 != h2) return int.MaxValue;
        int n = 0;
        for (int i = 0; i < x.Length; i += 3)
            if (x[i] != y[i] || x[i + 1] != y[i + 1] || x[i + 2] != y[i + 2]) n++;
        return n;
    }
    static uint[] table;
    static uint Crc(byte[] d, int off, int len, uint crc) {
        if (table == null) {
            table = new uint[256];
            for (uint n = 0; n < 256; n++) { uint c = n; for (int k = 0; k < 8; k++) c = (c & 1) != 0 ? 0xEDB88320u ^ (c >> 1) : c >> 1; table[n] = c; }
        }
        crc = ~crc;
        for (int i = 0; i < len; i++) crc = table[(crc ^ d[off + i]) & 0xFF] ^ (crc >> 8);
        return ~crc;
    }
    static void Chunk(Stream o, string type, byte[] data) {
        byte[] len = { (byte)(data.Length >> 24), (byte)(data.Length >> 16), (byte)(data.Length >> 8), (byte)data.Length };
        o.Write(len, 0, 4);
        byte[] t = System.Text.Encoding.ASCII.GetBytes(type);
        byte[] td = new byte[4 + data.Length];
        Array.Copy(t, td, 4); Array.Copy(data, 0, td, 4, data.Length);
        o.Write(td, 0, td.Length);
        uint c = Crc(td, 0, td.Length, 0);
        byte[] cb = { (byte)(c >> 24), (byte)(c >> 16), (byte)(c >> 8), (byte)c };
        o.Write(cb, 0, 4);
    }
    // PNG without compression (stored deflate blocks): big but needs no library.
    public static void ToPng(string ppm, string png) {
        int w, h; byte[] rgb = Read(ppm, out w, out h);
        byte[] raw = new byte[(w * 3 + 1) * h];
        for (int y = 0; y < h; y++) { raw[y * (w * 3 + 1)] = 0; Array.Copy(rgb, y * w * 3, raw, y * (w * 3 + 1) + 1, w * 3); }
        MemoryStream z = new MemoryStream();
        z.WriteByte(0x78); z.WriteByte(0x01);
        for (int pos = 0; pos < raw.Length; pos += 65535) {
            int n = Math.Min(65535, raw.Length - pos);
            z.WriteByte((byte)(pos + n >= raw.Length ? 1 : 0));
            z.WriteByte((byte)n); z.WriteByte((byte)(n >> 8)); z.WriteByte((byte)~n); z.WriteByte((byte)(~n >> 8));
            z.Write(raw, pos, n);
        }
        uint a = 1, b = 0;
        foreach (byte x in raw) { a = (a + x) % 65521; b = (b + a) % 65521; }
        uint ad = (b << 16) | a;
        z.WriteByte((byte)(ad >> 24)); z.WriteByte((byte)(ad >> 16)); z.WriteByte((byte)(ad >> 8)); z.WriteByte((byte)ad);
        using (FileStream o = File.Create(png)) {
            o.Write(new byte[] { 137, 80, 78, 71, 13, 10, 26, 10 }, 0, 8);
            byte[] ihdr = { (byte)(w >> 24), (byte)(w >> 16), (byte)(w >> 8), (byte)w, (byte)(h >> 24), (byte)(h >> 16), (byte)(h >> 8), (byte)h, 8, 2, 0, 0, 0 };
            Chunk(o, "IHDR", ihdr);
            Chunk(o, "IDAT", z.ToArray());
            Chunk(o, "IEND", new byte[0]);
        }
    }
}
'@

$serial = Join-Path $out 'serial.log'
$fs = Join-Path $out 'fs.img'
$f = [IO.File]::Create($fs); $f.SetLength(64MB); $f.Close()
$qargs = @('-accel', $Accel, '-m', '256M', '-display', 'none', '-no-reboot',
    '-serial', "file:$serial", '-qmp', "tcp:127.0.0.1:${Port},server,nowait",
    '-machine', 'pc,i8042=off',
    '-drive', "format=raw,file=$Image", '-drive', "format=raw,file=$fs,if=ide,index=2",
    '-device', 'qemu-xhci', '-device', 'usb-kbd', '-device', 'usb-mouse')
$proc = Start-Process -FilePath $qemu -ArgumentList $qargs -PassThru -WindowStyle Hidden
$results = New-Object System.Collections.Generic.List[string]
$failed = $false
function Check([bool]$ok, [string]$what) {
    $line = $(if ($ok) { 'PASS ' } else { 'FAIL ' }) + $what
    $script:results.Add($line); Write-Host $line
    if (-not $ok) { $script:failed = $true }
}
try {
    $deadline = (Get-Date).AddSeconds($BootTimeout)
    while ((Get-Date) -lt $deadline) {
        if ((Test-Path $serial) -and (Select-String -Path $serial -Pattern 'first desktop frame' -Quiet)) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not (Select-String -Path $serial -Pattern 'first desktop frame' -Quiet)) { throw 'o desktop nao subiu a tempo' }
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
    function KeyEvents([string[]]$codes, [bool]$down) {
        ($codes | ForEach-Object { '{"type":"key","data":{"down":' + $down.ToString().ToLower() + ',"key":{"type":"qcode","data":"' + $_ + '"}}}' }) -join ','
    }
    # Presses the keys together (modifiers first), releases them in reverse order.
    function Chord([string[]]$codes) {
        [void](Qmp ('{"execute":"input-send-event","arguments":{"events":[' + (KeyEvents $codes $true) + ']}}'))
        Start-Sleep -Milliseconds 80
        $rev = [string[]]($codes[($codes.Count - 1)..0])
        [void](Qmp ('{"execute":"input-send-event","arguments":{"events":[' + (KeyEvents $rev $false) + ']}}'))
        Start-Sleep -Milliseconds 120
    }
    function Rel([int]$dx, [int]$dy) {
        # steps of at most 100 counts, as lib.sh does
        while ($dx -ne 0 -or $dy -ne 0) {
            $sx = [Math]::Max(-100, [Math]::Min(100, $dx)); $sy = [Math]::Max(-100, [Math]::Min(100, $dy))
            [void](Qmp ('{"execute":"input-send-event","arguments":{"events":[{"type":"rel","data":{"axis":"x","value":' + $sx + '}},{"type":"rel","data":{"axis":"y","value":' + $sy + '}}]}}'))
            $dx -= $sx; $dy -= $sy
            Start-Sleep -Milliseconds 30
        }
        Start-Sleep -Milliseconds 300
    }
    function Click() {
        [void](Qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}')
        Start-Sleep -Milliseconds 80
        [void](Qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}')
        Start-Sleep -Milliseconds 120
    }
    function Shot([string]$name) {
        $ppm = Join-Path $out "$name.ppm"
        $j = $ppm -replace '\\', '\\\\'
        [void](Qmp ('{"execute":"screendump","arguments":{"filename":"' + $j + '"}}'))
        Start-Sleep -Milliseconds 1500
        [Shot]::ToPng($ppm, (Join-Path $out "$name.png"))
        return $ppm
    }

    Check ((Select-String -Path $serial -Pattern 'usb: slot \d+: boot keyboard' -Quiet) -and
           (Select-String -Path $serial -Pattern 'usb: slot \d+: boot mouse' -Quiet)) 'the guest found a USB keyboard and a USB mouse'

    $a = Shot 'a-idle'
    # Keyboard: Ctrl+Space opens Busca.
    Chord @('ctrl', 'spc'); Start-Sleep -Seconds 2
    $b = Shot 'b-search'
    $dKey = [Shot]::Diff($a, $b)
    Check ($dKey -gt 5000) "Ctrl+Space on the USB keyboard opened Busca ($dKey pixels changed)"
    Chord @('esc'); Start-Sleep -Seconds 2
    $c1 = Shot 'c-closed'
    $dBack = [Shot]::Diff($b, $c1)
    Check ($dBack -gt 5000) "Esc on the USB keyboard closed it ($dBack pixels changed)"

    # Mouse: from the screen centre (640,360) to the Calculator icon of the dock (678,684), then click. (The
    # desktop starts with a Terminal already open, so the Terminal icon would minimise it instead.)
    Rel (678 - 640) (684 - 360)
    Click; Start-Sleep -Seconds 4
    $d = Shot 'd-calculator'
    $dMouse = [Shot]::Diff($c1, $d)
    Check ($dMouse -gt 5000) "moving the USB mouse and clicking opened the Calculator ($dMouse pixels changed)"

    # Typing reaches the focused Calculator.
    foreach ($k in @('1', '2', '3')) { Chord @($k) }
    Start-Sleep -Seconds 1
    $e = Shot 'e-typed'
    $dType = [Shot]::Diff($d, $e)
    Check ($dType -gt 100) "typing on the USB keyboard reached the Calculator ($dType pixels changed)"

    $log = Get-Content $serial -Raw
    Check ($log -match 'usb: key 0x1d down') 'the log shows the Ctrl key press'
    Check ($log -match 'usb: mouse dx=\d+ dy=-?\d+ left=true') 'the log shows the click'
    Check ($log -notmatch 'panic') 'no panic in the serial log'
    [void](Qmp '{"execute":"quit"}')
    $c.Close()
}
catch {
    Check $false ("script error: " + $_.Exception.Message)
}
finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    $results | Set-Content (Join-Path $out 'w64-result.txt')
}
Write-Host "Resultado e capturas em $out"
if ($failed) { exit 1 }
