# 프로토타입 창 영역만 캡처한다(전체 화면 캡처 금지). 사용: shot.ps1 -Out <png>
param([string]$Out = "C:\Users\yepben\AppData\Local\Temp\claude\D--projects\27bd221c-d430-488f-a743-9aab92618ad5\scratchpad\shot.png")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class S {
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string t);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [StructLayout(LayoutKind.Sequential)] public struct R { public int l, t, r, b; }
}
"@
[void][S]::SetProcessDPIAware()
$h = (Get-Process -Name proto_slint | Select-Object -First 1).MainWindowHandle
$r = New-Object S+R
[void][S]::GetWindowRect($h, [ref]$r)
$w = $r.r - $r.l; $ht = $r.b - $r.t
"rect: $($r.l),$($r.t) ${w}x${ht}"
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.l, $r.t, 0, 0, (New-Object System.Drawing.Size $w, $ht))
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
