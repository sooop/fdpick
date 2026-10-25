<#
fdpick - fd + fzf 경로 검색기 (경로 복사 / 탐색기 열기)

필요: fd, fzf (0.50 이상 권장), PowerShell 7.3 이상

질의 형식:   [fd 옵션] -- [fuzzy 검색어]
  -e pdf -t f -- 보고서          PDF 파일만 대상으로 '보고서' 검색
  -t d --max-depth 3 -- proj     깊이 3 이하 디렉터리만
  !-H -I -t f -- log             '!'로 시작하면 기본 옵션 없이 지정한 옵션만 사용
  (-- 가 없으면 전체 입력을 fuzzy 검색어로 사용, 기본 옵션으로 만든 목록 대상)

키:
  Enter       선택 경로 복사 (Tab 으로 다중 선택 가능)
  Ctrl-D      디렉터리 경로 복사 (파일이면 부모 폴더)
  Ctrl-O      탐색기에서 열기 (파일이면 선택 상태로)
  Alt-Enter   기본 프로그램으로 열기
  Ctrl-R      캐시 무시하고 fd 재실행
  Esc         종료
#>
param(
    [switch]$Search,     # 내부용: fzf reload 에서 호출되어 목록을 출력
    [switch]$Force,      # 내부용: 캐시 무시
    [string]$Build       # 내부용: 백그라운드 fd 실행 (캐시 파일 경로)
)

# ---------------- 설정 ----------------
$CacheMinutes = 15                       # 이 시간이 지난 캐시는 백그라운드로 갱신 (갱신 전까지는 이전 결과 사용)
$DriveTypes   = @('Fixed')               # 'Removable', 'Network' 를 추가하면 해당 드라이브도 검색
$MaxEmptyList = 2000                     # 검색어가 없을 때 보여줄 최대 항목 수
# 각 드라이브의 최상위 폴더 중 검색에서 제외할 이름
$DenyTopLevel = @('Windows', 'Program Files', 'Program Files (x86)', 'ProgramData',
                  '$Recycle.Bin', 'System Volume Information', 'Recovery', 'PerfLogs',
                  '$WinREAgent', 'Config.Msi', 'Documents and Settings', '$SysReset')
# 항상 붙는 fd 기본 옵션 ('!' 로 시작하는 질의에서는 생략)
$DefaultFdArgs = @('--hidden', '--exclude .git', '--exclude node_modules',
                   '--exclude __pycache__', '--exclude AppData')
# --------------------------------------

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { }

$CacheDir = Join-Path $env:LOCALAPPDATA 'fdpick\cache'
New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null

function Split-Query([string]$q) {
    # 공백으로 둘러싸인 단독 '--' 를 기준으로 fd 옵션과 검색어를 분리
    $m = [regex]::Match($q, '(?:^|\s)--(?:\s|$)')
    if ($m.Success) {
        [pscustomobject]@{
            FdArgs  = $q.Substring(0, $m.Index).Trim()
            Pattern = $q.Substring($m.Index + $m.Length).Trim()
        }
    } else {
        [pscustomobject]@{ FdArgs = ''; Pattern = $q.Trim() }
    }
}

function Get-FdLine([string]$userArgs) {
    $useDefaults = $true
    if ($userArgs.StartsWith('!')) {
        $useDefaults = $false
        $userArgs = $userArgs.Substring(1).Trim()
    }
    $parts = [System.Collections.Generic.List[string]]::new()
    $parts.Add('--absolute-path')
    $parts.Add('--color never')
    if ($useDefaults) { $parts.AddRange([string[]]$DefaultFdArgs) }
    if ($userArgs) { $parts.Add($userArgs) }

    # 사용자가 --search-path 를 직접 주지 않으면 전체 드라이브의 최상위 폴더들을 검색 경로로 사용
    if ($userArgs -notmatch '(^|\s)--search-path(\s|=)') {
        foreach ($d in [IO.DriveInfo]::GetDrives()) {
            if (($DriveTypes -contains $d.DriveType.ToString()) -and $d.IsReady) {
                $tops = Get-ChildItem -LiteralPath $d.RootDirectory.FullName -Directory -Force -ErrorAction SilentlyContinue
                foreach ($dir in $tops) {
                    if ($DenyTopLevel -notcontains $dir.Name) {
                        $parts.Add('--search-path "' + $dir.FullName + '"')
                    }
                }
            }
        }
    }
    $parts -join ' '
}

function Get-CachePath([string]$line) {
    $sha = [Security.Cryptography.SHA1]::Create()
    $h = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($line))).Replace('-', '')
    Join-Path $CacheDir "$h.txt"
}

function Start-CacheBuild([string]$line, [string]$cache) {
    # fd 를 reload 프로세스와 분리된 백그라운드 프로세스로 실행한다.
    # (키 입력마다 reload 가 취소되어도 fd 실행이 끊기지 않게 하기 위함)
    $lock = "$cache.lock"
    if ((Test-Path -LiteralPath $lock) -and
        ((Get-Item -LiteralPath $lock).LastWriteTime -lt (Get-Date).AddMinutes(-5))) {
        Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
    }
    try { $fs = [IO.File]::Open($lock, [IO.FileMode]::CreateNew) } catch { return }  # 이미 빌드 중
    $fs.Close()
    try {
        $fd = (Get-Command fd -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        [IO.File]::WriteAllLines("$cache.line", @($fd, $line), [Text.UTF8Encoding]::new($false))
        $self = $PSCommandPath
        $pwsh = [Environment]::ProcessPath
        try {
            $cmd = '"{0}" -NoProfile -WindowStyle Hidden -File "{1}" -Build "{2}"' -f $pwsh, $self, $cache
            $si = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0 }
            $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create `
                -Arguments @{ CommandLine = $cmd; ProcessStartupInformation = $si }
            if ($r.ReturnValue -ne 0) { throw "CIM Create 실패: $($r.ReturnValue)" }
        } catch {
            $argLine = '-NoProfile -WindowStyle Hidden -File "{0}" -Build "{1}"' -f $self, $cache
            Start-Process -FilePath $pwsh -ArgumentList $argLine -WindowStyle Hidden
        }
    } catch {
        Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
        throw
    }
}

# ---------------- 모드 1: 백그라운드 fd 실행 ----------------
if ($Build) {
    $cache = $Build
    try {
        $lines = [IO.File]::ReadAllLines("$cache.line", [Text.Encoding]::UTF8)
        $tmp = "$cache.build"
        Start-Process -FilePath $lines[0] -ArgumentList $lines[1] -NoNewWindow -Wait `
            -RedirectStandardOutput $tmp -RedirectStandardError "$cache.err"
        Move-Item -LiteralPath $tmp -Destination $cache -Force
    } finally {
        foreach ($s in '.lock', '.err', '.line', '.build') {
            Remove-Item -LiteralPath "$cache$s" -Force -ErrorAction SilentlyContinue
        }
    }
    exit 0
}

# ---------------- 모드 2: fzf reload 용 검색 ----------------
if ($Search) {
    try {
        $sq = Split-Query ([string]$env:FZF_QUERY)
        $line = Get-FdLine $sq.FdArgs
        $cache = Get-CachePath $line
        if ($Force) { Remove-Item -LiteralPath $cache -Force -ErrorAction SilentlyContinue }

        $stale = -not (Test-Path -LiteralPath $cache)
        if (-not $stale) {
            $stale = (Get-Item -LiteralPath $cache).LastWriteTime -lt (Get-Date).AddMinutes(-$CacheMinutes)
        }
        if ($stale) { Start-CacheBuild $line $cache }

        # 캐시가 아직 없으면 빌드가 끝날 때까지 대기 (오래된 캐시가 있으면 그대로 사용)
        $lock = "$cache.lock"
        $deadline = (Get-Date).AddSeconds(300)
        while (-not (Test-Path -LiteralPath $cache) -and (Test-Path -LiteralPath $lock) -and ((Get-Date) -lt $deadline)) {
            Start-Sleep -Milliseconds 150
        }
        if (-not (Test-Path -LiteralPath $cache)) {
            throw 'fd 결과를 만들지 못했습니다. fd 옵션을 확인하세요.'
        }

        if ($sq.Pattern) {
            $fzf = (Get-Command fzf -CommandType Application | Select-Object -First 1).Source
            $psi = [Diagnostics.ProcessStartInfo]::new($fzf)
            foreach ($a in '--filter', $sq.Pattern, '--scheme=path', '--tiebreak=pathname,length') {
                $psi.ArgumentList.Add($a)
            }
            $psi.UseShellExecute = $false
            $psi.RedirectStandardInput = $true
            $p = [Diagnostics.Process]::Start($psi)
            $fs = [IO.File]::Open($cache, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
            try { $fs.CopyTo($p.StandardInput.BaseStream) } finally { $fs.Close() }
            $p.StandardInput.Close()
            $p.WaitForExit()
        } else {
            Get-Content -LiteralPath $cache -TotalCount $MaxEmptyList
        }
    } catch {
        '[오류] ' + $_.Exception.Message
    }
    exit 0
}

# ---------------- 모드 3: 대화형 ----------------
foreach ($c in 'fd', 'fzf') {
    if (-not (Get-Command $c -CommandType Application -ErrorAction SilentlyContinue)) {
        Write-Host "$c 를 찾을 수 없습니다. winget install sharkdp.fd junegunn.fzf"
        Read-Host '엔터를 누르면 닫힙니다' | Out-Null
        exit 1
    }
}

# 오래된 캐시 정리
Get-ChildItem -LiteralPath $CacheDir -File -ErrorAction SilentlyContinue |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-1) } |
    Remove-Item -Force -ErrorAction SilentlyContinue

function Get-CleanPath([string]$p) {
    $p = $p.Trim()
    if ($p.Length -gt 3) { $p = $p.TrimEnd('\', '/') }   # fd 가 디렉터리 끝에 붙이는 구분자 제거
    $p
}
function Get-DirPath([string]$p) {
    if (Test-Path -LiteralPath $p -PathType Container) { $p } else { Split-Path -LiteralPath $p -Parent }
}

$env:FDPICK_SELF = $PSCommandPath
$env:FZF_DEFAULT_COMMAND = 'rem'     # 시작 시 fzf 가 기본 목록(dir /s)을 만들지 않도록 무력화
$helper = 'pwsh -NoProfile -File "%FDPICK_SELF%" -Search'
$header = "Enter 경로복사 | Ctrl-D 폴더경로 | Ctrl-O 탐색기 | Alt-Enter 열기 | Ctrl-R 새로고침`n" +
          "형식: [fd옵션] -- 검색어   예) -e pdf -t f -- 보고서    '!' 로 시작하면 기본옵션 제외"

$fzfArgs = @(
    '--disabled', '--multi', '--layout=reverse', '--info=inline',
    '--prompt', '> ',
    '--header', $header,
    '--expect', 'ctrl-d,ctrl-o,alt-enter',
    '--bind', "start:reload:$helper",
    '--bind', "change:reload:$helper",
    '--bind', "ctrl-r:reload:$helper -Force"
)
$out = & fzf @fzfArgs

$all = @($out | Where-Object { $null -ne $_ })
if ($all.Count -lt 2) { exit 0 }
$key = $all[0]
$paths = @($all[1..($all.Count - 1)] |
    Where-Object { $_ -and ($_ -notmatch '^\[오류\]') } |
    ForEach-Object { Get-CleanPath $_ })
if ($paths.Count -eq 0) { exit 0 }

switch ($key) {
    'ctrl-d' {
        $dirs = $paths | ForEach-Object { Get-DirPath $_ } | Select-Object -Unique
        Set-Clipboard -Value ($dirs -join "`r`n")
    }
    'ctrl-o' {
        $p = $paths[0]
        if (Test-Path -LiteralPath $p -PathType Container) {
            Start-Process -FilePath 'explorer.exe' -ArgumentList ('"{0}"' -f $p)
        } else {
            Start-Process -FilePath 'explorer.exe' -ArgumentList ('/select,"{0}"' -f $p)
        }
    }
    'alt-enter' {
        Start-Process -FilePath $paths[0]
    }
    default {
        Set-Clipboard -Value ($paths -join "`r`n")
    }
}
exit 0
