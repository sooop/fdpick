# 프로토타입 메모리 측정: 지정한 이름의 프로세스와 모든 하위 프로세스의 메모리를 합산한다.
# 사용: powershell -NoProfile -File measure.ps1 -Name proto_egui -Label "idle-visible"
# 출력은 영문(콘솔 코드페이지 문제 회피). priv = 개인 메모리(공유 페이지 제외), ws = 작업 집합.
param([Parameter(Mandatory = $true)][string]$Name, [string]$Label = "")
$root = (Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1).Id
if (-not $root) { "$Label : process '$Name' not found"; return }
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
$ids = New-Object System.Collections.Generic.List[int]
$ids.Add([int]$root)
for ($i = 0; $i -lt $ids.Count; $i++) {
    foreach ($c in $all) {
        if ($c.ParentProcessId -eq $ids[$i] -and -not $ids.Contains([int]$c.ProcessId)) { $ids.Add([int]$c.ProcessId) }
    }
}
$ws = 0; $pv = 0; $rows = @()
foreach ($id in $ids) {
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($p) {
        $ws += $p.WorkingSet64; $pv += $p.PrivateMemorySize64
        $rows += ("  {0,-20} pid={1,-6} ws={2,7:N1}MB priv={3,7:N1}MB" -f $p.ProcessName, $p.Id, ($p.WorkingSet64 / 1MB), ($p.PrivateMemorySize64 / 1MB))
    }
}
"== {0}: procs={1} ws_total={2:N1}MB priv_total={3:N1}MB" -f $Label, $ids.Count, ($ws / 1MB), ($pv / 1MB)
$rows
