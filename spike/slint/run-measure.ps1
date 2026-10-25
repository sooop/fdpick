# Slint 프로토타입 측정 자동화. 사용: powershell -NoProfile -File run-measure.ps1 -Exe <경로> -Label sw [-Extra "--rows 2000"]
param([Parameter(Mandatory = $true)][string]$Exe, [string]$Label = "", [string]$Extra = "", [int]$Vk = 0x46)
$ErrorActionPreference = "Stop"
$measure = "D:\projects\fdpick\spike\measure.ps1"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W {
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string t);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] i, int size);
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public KI ki; public long pad; }
    [StructLayout(LayoutKind.Sequential)] public struct KI { public ushort vk; public ushort scan; public uint flags; public uint time; public IntPtr extra; }
    public static void Key(ushort vk, bool up) {
        INPUT[] a = new INPUT[1];
        a[0].type = 1; a[0].ki.vk = vk; a[0].ki.flags = up ? 2u : 0u;
        SendInput(1, a, Marshal.SizeOf(typeof(INPUT)));
    }
    public static void Hotkey(ushort vk) { // Ctrl+Alt+<vk>
        Key(0x11, false); Key(0x12, false); Key(vk, false);
        Key(vk, true); Key(0x12, true); Key(0x11, true);
    }
}
"@
function Kill-Proto { Get-Process -Name proto_slint -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep -Milliseconds 300 }
$script:H = [IntPtr]::Zero
$script:P = $null
function Wait-Visible($timeoutMs) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $script:P.Refresh(); $hh = $script:P.MainWindowHandle
        if ($hh -ne [IntPtr]::Zero -and [W]::IsWindowVisible($hh)) { return $sw.Elapsed.TotalMilliseconds }
        Start-Sleep -Milliseconds 10
    }
    return -1
}
Kill-Proto
# 1) 시작 시간 3회: 프로세스 시작부터 MainWindowHandle 생성 + IsWindowVisible 까지(10ms 폴링)
$starts = @()
for ($n = 0; $n -lt 3; $n++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $Exe -ArgumentList $Extra -PassThru
    $t = -1
    while ($sw.ElapsedMilliseconds -lt 10000) {
        $p.Refresh()
        if ($p.MainWindowHandle -ne [IntPtr]::Zero -and [W]::IsWindowVisible($p.MainWindowHandle)) { $t = $sw.Elapsed.TotalMilliseconds; break }
        Start-Sleep -Milliseconds 10
    }
    $starts += $t
    Kill-Proto
}
"[$Label] startup_ms: " + (($starts | ForEach-Object { "{0:N0}" -f $_ }) -join ", ") + "  avg=" + ("{0:N0}" -f ($starts | Measure-Object -Average).Average)

# 2) 시작 5초 후 보임/유휴 메모리
$p = Start-Process -FilePath $Exe -ArgumentList $Extra -PassThru
Start-Sleep -Seconds 5
& powershell -NoProfile -File $measure -Name proto_slint -Label "$Label visible-idle(5s)"
Kill-Proto

# 3) --hide-after 5 후 10초 뒤, 단축키 재표시 5초 뒤 메모리 + 숨김 중 CPU + 단축키 지연
$p = Start-Process -FilePath $Exe -ArgumentList "$Extra --hide-after 5" -PassThru
Start-Sleep -Seconds 1
$script:P = $p; $p.Refresh(); $script:H = $p.MainWindowHandle
Start-Sleep -Seconds 14
"[$Label] after hide-after: visible=" + [W]::IsWindowVisible($script:H) + " handle=" + $script:H
& powershell -NoProfile -File $measure -Name proto_slint -Label "$Label hidden(+10s)"
# 숨김 중 CPU: 5초간 누적 CPU 시간 변화
$p.Refresh(); $c0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 5
$p.Refresh(); $c1 = $p.TotalProcessorTime.TotalMilliseconds
"[$Label] hidden cpu over 5s: {0:N1} ms" -f ($c1 - $c0)
# 첫 재표시
$sw0 = [Diagnostics.Stopwatch]::StartNew()
[W]::Hotkey($Vk)
$null = Wait-Visible 3000
$lat = $sw0.Elapsed.TotalMilliseconds
"[$Label] first hotkey show latency_ms: {0:N0}" -f $lat
Start-Sleep -Seconds 5
& powershell -NoProfile -File $measure -Name proto_slint -Label "$Label reshown(+5s)"
# 지연 3회: 숨김(단축키) -> 표시(단축키) 반복
$lats = @()
for ($n = 0; $n -lt 3; $n++) {
    [W]::Hotkey($Vk); Start-Sleep -Milliseconds 1000
    $p.Refresh(); if ($p.MainWindowHandle -ne [IntPtr]::Zero -and [W]::IsWindowVisible($p.MainWindowHandle)) { "[$Label] warn: not hidden after toggle" }
    $sw0 = [Diagnostics.Stopwatch]::StartNew()
    [W]::Hotkey($Vk)
    $null = Wait-Visible 3000
    $lats += $sw0.Elapsed.TotalMilliseconds
    Start-Sleep -Milliseconds 800
}
"[$Label] hotkey show latency_ms (3x): " + (($lats | ForEach-Object { "{0:N0}" -f $_ }) -join ", ") + "  avg=" + ("{0:N0}" -f ($lats | Measure-Object -Average).Average)
Kill-Proto
