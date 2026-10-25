//! fd 실행기: fd 를 스트리밍으로 돌려 결과 경로를 배치 이벤트로 흘려보낸다.
//!
//! - 비동기 런타임 없이 std 스레드 + mpsc 만 쓴다.
//! - 프로세스는 Windows Job Object(KILL_ON_JOB_CLOSE)에 넣어 취소/앱 종료 시 트리째 죽인다.
//! - stdout 은 NUL 구분 바이트 스트림이며 `NulSplitter` 가 청크 경계를 이어 붙인다.

use std::ffi::c_void;
use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetLogicalDrives,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_SUSPENDED: u32 = 0x0000_0004;

// GetDriveTypeW 반환값(windows-sys 에서 별도 feature 가 필요해 상수로 둔다)
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

/// 경로 배치 크기 상한(개수)
const BATCH_MAX_ITEMS: usize = 4096;
/// 경로 배치 flush 주기
const BATCH_INTERVAL: Duration = Duration::from_millis(20);
/// StderrSummary 갱신 최소 간격
const STDERR_EMIT_INTERVAL: Duration = Duration::from_millis(200);
/// stderr 기타 줄 최대 보관 수
const STDERR_OTHER_MAX: usize = 20;
/// 한 번 실행에 넘기는 명령줄 길이 상한(CreateProcess 한계 32767 보다 여유 있게)
const CMDLINE_BUDGET: usize = 20_000;

/// 최상위 폴더 제외 이름(대소문자 무시)
pub const DEFAULT_DENY_TOP: &[&str] = &[
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
    "$Recycle.Bin",
    "System Volume Information",
    "Recovery",
    "PerfLogs",
    "$WinREAgent",
    "Config.Msi",
    "Documents and Settings",
    "$SysReset",
];

// ---------------------------------------------------------------------------
// 설정 / 이벤트
// ---------------------------------------------------------------------------

/// 검색 대상 드라이브 종류
#[derive(Clone, Debug)]
pub struct DriveTypes {
    pub fixed: bool,
    pub removable: bool,
    pub network: bool,
}

impl Default for DriveTypes {
    fn default() -> Self {
        Self { fixed: true, removable: false, network: false }
    }
}

#[derive(Clone, Debug)]
pub struct FdConfig {
    /// fd 실행 파일 경로(없으면 PATH 에서 fd.exe 탐색)
    pub exe: Option<PathBuf>,
    /// use_defaults 일 때 붙는 기본 옵션
    pub default_args: Vec<String>,
    /// 드라이브 최상위에서 제외할 폴더 이름
    pub deny_top: Vec<String>,
    pub drive_types: DriveTypes,
}

impl Default for FdConfig {
    fn default() -> Self {
        let d = |s: &str| s.to_string();
        Self {
            exe: None,
            default_args: vec![
                d("--hidden"),
                d("--exclude"),
                d(".git"),
                d("--exclude"),
                d("node_modules"),
                d("--exclude"),
                d("__pycache__"),
                d("--exclude"),
                d("AppData"),
            ],
            deny_top: DEFAULT_DENY_TOP.iter().map(|s| s.to_string()).collect(),
            drive_types: DriveTypes::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FdEvent {
    /// 결과 경로 배치(최대 4096개 또는 20ms 단위)
    Paths(Vec<String>),
    /// stderr 요약(변경 시 갱신, 누적값)
    StderrSummary { permission_errors: usize, other: Vec<String> },
    /// 정상 종료(취소 시에는 오지 않는다)
    Done { exit_ok: bool, elapsed_ms: u64, total: u64, lossy_count: u64 },
    /// fd 를 못 찾았거나 결과 없이 비정상 종료
    Failed(String),
}

// ---------------------------------------------------------------------------
// NUL 분리기
// ---------------------------------------------------------------------------

/// NUL 구분 바이트 스트림을 항목으로 나눈다. 청크 경계에 걸린 항목은 이어 붙인다.
/// 빈 항목은 버린다. UTF-8 이 아닌 항목은 lossy 변환하고 `lossy_count` 를 올린다.
#[derive(Default, Debug)]
pub struct NulSplitter {
    buf: Vec<u8>,
    pub lossy_count: u64,
}

impl NulSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    fn emit(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        if bytes.is_empty() {
            return;
        }
        match String::from_utf8_lossy(bytes) {
            std::borrow::Cow::Borrowed(s) => out.push(s.to_string()),
            std::borrow::Cow::Owned(s) => {
                self.lossy_count += 1;
                out.push(s);
            }
        }
    }

    /// 청크를 넣고 완성된 항목을 `out` 에 추가한다.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<String>) {
        let mut start = 0;
        while let Some(p) = chunk[start..].iter().position(|&b| b == 0) {
            let end = start + p;
            if self.buf.is_empty() {
                self.emit(&chunk[start..end], out);
            } else {
                self.buf.extend_from_slice(&chunk[start..end]);
                let taken = std::mem::take(&mut self.buf);
                self.emit(&taken, out);
            }
            start = end + 1;
        }
        self.buf.extend_from_slice(&chunk[start..]);
    }

    /// 스트림 끝: NUL 로 끝나지 않은 꼬리를 항목으로 내보낸다.
    pub fn finish(&mut self, out: &mut Vec<String>) {
        let taken = std::mem::take(&mut self.buf);
        self.emit(&taken, out);
    }
}

// ---------------------------------------------------------------------------
// 검색 루트 계산(순수 함수)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirItem {
    pub name: String,
    /// 심볼릭 링크/정션이 아닌 실제 디렉터리인가
    pub is_dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPlan {
    /// `--search-path` 로 넘길 최상위 폴더들(제외 목록 반영)
    pub top_dirs: Vec<PathBuf>,
    /// 드라이브 루트(루트 바로 아래 파일용 depth-1 검색 대상)
    pub drive_roots: Vec<PathBuf>,
}

/// 사용자 인자에 `--search-path`(공백 또는 `=` 형태)가 있는가
pub fn has_search_path(user_args: &[String]) -> bool {
    user_args.iter().any(|a| a == "--search-path" || a.starts_with("--search-path="))
}

/// 사용자가 깊이를 직접 지정했는가(그러면 depth-1 보충 검색을 생략한다)
fn has_depth_option(user_args: &[String]) -> bool {
    user_args.iter().any(|a| {
        a == "-d"
            || a == "--max-depth"
            || a == "--maxdepth"
            || a == "--exact-depth"
            || a == "--min-depth"
            || a.starts_with("--max-depth=")
            || a.starts_with("--maxdepth=")
            || a.starts_with("--exact-depth=")
            || a.starts_with("--min-depth=")
            || (a.starts_with("-d") && a.len() > 2 && a[2..].chars().all(|c| c.is_ascii_digit()))
    })
}

/// 드라이브 목록과 폴더 읽기 함수로 검색 계획을 계산한다.
/// 폴더를 못 읽는 드라이브도 루트 depth-1 검색 대상에는 남긴다.
pub fn compute_plan(
    drives: &[PathBuf],
    read_dir: &dyn Fn(&Path) -> io::Result<Vec<DirItem>>,
    deny_top: &[String],
) -> SearchPlan {
    let deny: Vec<String> = deny_top.iter().map(|s| s.to_lowercase()).collect();
    let mut plan = SearchPlan::default();
    for root in drives {
        plan.drive_roots.push(root.clone());
        let Ok(items) = read_dir(root) else { continue };
        for it in items {
            if !it.is_dir || deny.contains(&it.name.to_lowercase()) {
                continue;
            }
            plan.top_dirs.push(root.join(&it.name));
        }
    }
    plan
}

/// 실제 파일시스템으로 폴더를 읽는다(`compute_plan` 에 넘기는 기본 구현).
pub fn read_dir_real(p: &Path) -> io::Result<Vec<DirItem>> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(p)? {
        let Ok(e) = e else { continue };
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        v.push(DirItem { name: e.file_name().to_string_lossy().into_owned(), is_dir });
    }
    Ok(v)
}

/// 설정된 종류의 드라이브 루트(`C:\` 형태)를 열거한다.
pub fn enumerate_drives(types: &DriveTypes) -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    let mut v = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let t = unsafe { GetDriveTypeW(wide.as_ptr()) };
        let ok = (t == DRIVE_FIXED && types.fixed)
            || (t == DRIVE_REMOVABLE && types.removable)
            || (t == DRIVE_REMOTE && types.network);
        if ok {
            v.push(PathBuf::from(root));
        }
    }
    v
}

// ---------------------------------------------------------------------------
// 인자 조립(순수 함수)
// ---------------------------------------------------------------------------

fn base_args(cfg: &FdConfig, use_defaults: bool) -> Vec<String> {
    let mut a: Vec<String> =
        ["--absolute-path", "--color", "never", "--print0"].iter().map(|s| s.to_string()).collect();
    if use_defaults {
        a.extend(cfg.default_args.iter().cloned());
    }
    a
}

/// 실행할 fd 인자 묶음 목록을 계산한다. 각 묶음이 fd 프로세스 하나다.
/// 우리 옵션을 사용자 인자보다 앞에 둔다(사용자가 `--` 를 써도 안전).
///
/// `plan` 이 None 이면 사용자 `--search-path` 를 그대로 존중해 한 번만 실행한다.
pub fn build_runs(
    cfg: &FdConfig,
    user_args: &[String],
    use_defaults: bool,
    plan: Option<&SearchPlan>,
) -> Vec<Vec<String>> {
    let base = base_args(cfg, use_defaults);
    let Some(plan) = plan else {
        let mut a = base;
        a.extend(user_args.iter().cloned());
        return vec![a];
    };

    let mut runs = Vec::new();
    // 1) 최상위 폴더별 전체 검색(명령줄 길이 예산 내에서 분할)
    let mut chunk: Vec<String> = Vec::new();
    let mut len = 0usize;
    let flush = |chunk: &mut Vec<String>, runs: &mut Vec<Vec<String>>| {
        if chunk.is_empty() {
            return;
        }
        let mut a = base.clone();
        for p in chunk.drain(..) {
            a.push("--search-path".into());
            a.push(p);
        }
        a.extend(user_args.iter().cloned());
        runs.push(a);
    };
    for d in &plan.top_dirs {
        let s = d.to_string_lossy().into_owned();
        if len + s.len() + 16 > CMDLINE_BUDGET {
            flush(&mut chunk, &mut runs);
            len = 0;
        }
        len += s.len() + 16;
        chunk.push(s);
    }
    flush(&mut chunk, &mut runs);

    // 2) 드라이브 루트 바로 아래 항목(파일 + 최상위 폴더 자체)을 depth-1 로 보충.
    //    제외 목록 폴더가 섞이지 않도록 이름을 --exclude 로 건다.
    if !plan.drive_roots.is_empty() && !has_depth_option(user_args) {
        let mut a = base.clone();
        a.push("--max-depth".into());
        a.push("1".into());
        for n in &cfg.deny_top {
            a.push("--exclude".into());
            a.push(n.clone());
        }
        for r in &plan.drive_roots {
            a.push("--search-path".into());
            a.push(r.to_string_lossy().into_owned());
        }
        a.extend(user_args.iter().cloned());
        runs.push(a);
    }
    runs
}

/// 드라이브 열거/폴더 읽기를 주입받아 실행 묶음을 계산한다.
pub fn plan_runs(
    cfg: &FdConfig,
    user_args: &[String],
    use_defaults: bool,
    drives: &[PathBuf],
    read_dir: &dyn Fn(&Path) -> io::Result<Vec<DirItem>>,
) -> Result<Vec<Vec<String>>, String> {
    if has_search_path(user_args) {
        return Ok(build_runs(cfg, user_args, use_defaults, None));
    }
    if drives.is_empty() {
        return Err("검색할 드라이브가 없다(고정 드라이브를 찾지 못함)".into());
    }
    let plan = compute_plan(drives, read_dir, &cfg.deny_top);
    Ok(build_runs(cfg, user_args, use_defaults, Some(&plan)))
}

// ---------------------------------------------------------------------------
// fd 실행 파일 탐색
// ---------------------------------------------------------------------------

/// cfg.exe 우선, 없으면 PATH 환경변수를 직접 훑어 fd.exe 를 찾는다.
pub fn resolve_exe(cfg: &FdConfig) -> Result<PathBuf, String> {
    if let Some(p) = &cfg.exe {
        return if p.is_file() {
            Ok(p.clone())
        } else {
            Err(format!("지정한 fd 실행 파일을 찾을 수 없다: {}", p.display()))
        };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let c = dir.join("fd.exe");
        if c.is_file() {
            return Ok(c);
        }
    }
    Err("PATH 에서 fd.exe 를 찾을 수 없다. fd 를 설치하거나 설정에서 경로를 지정하라".into())
}

// ---------------------------------------------------------------------------
// Job Object
// ---------------------------------------------------------------------------

struct JobHandle(HANDLE);
// HANDLE 은 스레드 간 공유해도 안전한 커널 핸들이다.
unsafe impl Send for JobHandle {}
unsafe impl Sync for JobHandle {}

impl Drop for JobHandle {
    fn drop(&mut self) {
        // 마지막 핸들이 닫히면 KILL_ON_JOB_CLOSE 로 남은 프로세스가 모두 종료된다.
        unsafe { CloseHandle(self.0) };
    }
}

fn create_job() -> Result<JobHandle, String> {
    unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return Err(format!("Job Object 생성 실패: {}", io::Error::last_os_error()));
        }
        let job = JobHandle(h);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            return Err(format!("Job Object 설정 실패: {}", io::Error::last_os_error()));
        }
        Ok(job)
    }
}

/// 일시중지로 시작된 프로세스의 스레드를 재개한다.
/// std 는 메인 스레드 핸들을 노출하지 않으므로 ToolHelp 로 pid 의 스레드를 찾는다.
fn resume_process(pid: u32) -> Result<(), String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap.is_null() || snap as isize == -1 {
            return Err(format!("스레드 스냅샷 실패: {}", io::Error::last_os_error()));
        }
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut ok = Thread32First(snap, &mut te);
        while ok != 0 {
            if te.th32OwnerProcessID == pid {
                let th = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                if !th.is_null() {
                    if ResumeThread(th) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(th);
                }
            }
            ok = Thread32Next(snap, &mut te);
        }
        CloseHandle(snap);
        if resumed == 0 {
            return Err("프로세스 스레드를 재개하지 못했다".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 공유 상태 / 핸들
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StderrState {
    permission_errors: usize,
    other: Vec<String>,
    /// 실패 메시지용: 종류 무관 앞쪽 줄
    head: Vec<String>,
    dirty: bool,
}

fn is_permission_line(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("permission denied")
        || l.contains("access is denied")
        || l.contains("액세스가 거부")
        || l.contains("os error 5")
}

struct Shared {
    /// true 면 취소됨. 이벤트 전송은 이 락 안에서만 하므로 cancel() 반환 후 이벤트가 없다.
    gate: Mutex<bool>,
    job: JobHandle,
    pids: Mutex<Vec<u32>>,
    stderr: Mutex<StderrState>,
    killed: AtomicBool,
}

impl Shared {
    /// 취소되지 않았을 때만 전송. 수신측이 닫혔으면 false.
    fn send(&self, tx: &Sender<FdEvent>, ev: FdEvent) -> bool {
        let g = self.gate.lock().unwrap();
        if *g {
            return false;
        }
        tx.send(ev).is_ok()
    }

    fn kill_all(&self) {
        if !self.killed.swap(true, Ordering::SeqCst) {
            unsafe { TerminateJobObject(self.job.0, 1) };
        }
    }

    fn is_cancelled(&self) -> bool {
        *self.gate.lock().unwrap()
    }
}

/// 실행 중인 fd 작업 핸들. Drop 시 취소한다.
pub struct FdJob {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl FdJob {
    /// fd 프로세스 트리를 종료하고 스레드를 정리한다. 반환 후에는 이벤트가 오지 않는다.
    pub fn cancel(&self) {
        {
            let mut g = self.shared.gate.lock().unwrap();
            *g = true;
        }
        self.shared.kill_all();
        let th = self.thread.lock().unwrap().take();
        if let Some(th) = th {
            // 파이프가 닫히면 스레드가 곧 끝난다. 혹시 모를 정지에 대비해 대기를 제한한다.
            let t0 = Instant::now();
            while !th.is_finished() && t0.elapsed() < Duration::from_secs(3) {
                thread::sleep(Duration::from_millis(1));
            }
            if th.is_finished() {
                let _ = th.join();
            }
        }
    }

    /// 지금까지 띄운 fd 프로세스 pid(스코프 shim 이면 shim pid 다)
    pub fn child_pids(&self) -> Vec<u32> {
        self.shared.pids.lock().unwrap().clone()
    }

    /// Job 안에서 살아 있는 프로세스 수(자손 포함)
    pub fn active_processes(&self) -> u32 {
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                self.shared.job.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                0
            } else {
                info.ActiveProcesses
            }
        }
    }

    /// 작업 스레드가 끝났는가
    pub fn is_finished(&self) -> bool {
        match &*self.thread.lock().unwrap() {
            Some(t) => t.is_finished(),
            None => true,
        }
    }
}

impl Drop for FdJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

// ---------------------------------------------------------------------------
// 시작
// ---------------------------------------------------------------------------

/// fd 를 시작한다. 실행 파일 탐색·루트 계산·프로세스 실행 실패는 `FdEvent::Failed` 로 온다.
/// `Err` 는 Job Object/스레드 생성 같은 내부 실패에만 쓴다.
pub fn start(
    cfg: &FdConfig,
    user_args: Vec<String>,
    use_defaults: bool,
    tx: Sender<FdEvent>,
) -> Result<FdJob, String> {
    let job = create_job()?;
    let shared = Arc::new(Shared {
        gate: Mutex::new(false),
        job,
        pids: Mutex::new(Vec::new()),
        stderr: Mutex::new(StderrState::default()),
        killed: AtomicBool::new(false),
    });
    let cfg = cfg.clone();
    let sh = shared.clone();
    let th = thread::Builder::new()
        .name("fd-coordinator".into())
        .spawn(move || coordinate(sh, cfg, user_args, use_defaults, tx))
        .map_err(|e| format!("스레드 생성 실패: {e}"))?;
    Ok(FdJob { shared, thread: Mutex::new(Some(th)) })
}

fn failed_message(sh: &Shared, prefix: &str) -> String {
    let st = sh.stderr.lock().unwrap();
    if st.head.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n{}", st.head.join("\n"))
    }
}

fn emit_stderr_if_dirty(sh: &Shared, tx: &Sender<FdEvent>) -> bool {
    let ev = {
        let mut st = sh.stderr.lock().unwrap();
        if !st.dirty {
            return true;
        }
        st.dirty = false;
        FdEvent::StderrSummary { permission_errors: st.permission_errors, other: st.other.clone() }
    };
    sh.send(tx, ev)
}

fn coordinate(
    sh: Arc<Shared>,
    cfg: FdConfig,
    user_args: Vec<String>,
    use_defaults: bool,
    tx: Sender<FdEvent>,
) {
    let started = Instant::now();
    let exe = match resolve_exe(&cfg) {
        Ok(e) => e,
        Err(m) => {
            sh.send(&tx, FdEvent::Failed(m));
            return;
        }
    };
    let drives =
        if has_search_path(&user_args) { Vec::new() } else { enumerate_drives(&cfg.drive_types) };
    let runs = match plan_runs(&cfg, &user_args, use_defaults, &drives, &read_dir_real) {
        Ok(r) => r,
        Err(m) => {
            sh.send(&tx, FdEvent::Failed(m));
            return;
        }
    };

    let mut total: u64 = 0;
    let mut lossy: u64 = 0;
    let mut all_ok = true;
    for args in runs {
        if sh.is_cancelled() {
            return;
        }
        match run_one(&sh, &exe, &args, &tx, &mut total, &mut lossy) {
            RunResult::Aborted => return,
            RunResult::Ok => {}
            RunResult::Failed(status) => {
                if total == 0 {
                    // 결과 없이 비정상 종료: 즉시 실패로 보고
                    let msg = failed_message(&sh, &format!("fd 가 비정상 종료했다({status})"));
                    sh.send(&tx, FdEvent::Failed(msg));
                    return;
                }
                all_ok = false;
            }
            RunResult::SpawnError(m) => {
                sh.send(&tx, FdEvent::Failed(m));
                return;
            }
        }
    }
    if !emit_stderr_if_dirty(&sh, &tx) {
        return;
    }
    sh.send(
        &tx,
        FdEvent::Done {
            exit_ok: all_ok,
            elapsed_ms: started.elapsed().as_millis() as u64,
            total,
            lossy_count: lossy,
        },
    );
}

enum RunResult {
    Ok,
    Failed(String),
    SpawnError(String),
    /// 취소 또는 수신측 종료: 아무 이벤트도 더 보내지 않는다
    Aborted,
}

/// fd 프로세스를 일시중지 상태로 띄워 Job 에 넣은 뒤 재개한다.
/// CREATE_SUSPENDED 덕에 spawn 과 Assign 사이 경쟁이 없다(자식이 만든 자손도 모두 Job 에 속한다).
fn spawn_in_job(sh: &Shared, exe: &Path, args: &[String]) -> Result<Child, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut child = cmd.spawn().map_err(|e| format!("fd 실행 실패({}): {e}", exe.display()))?;
    let ok = unsafe { AssignProcessToJobObject(sh.job.0, child.as_raw_handle() as HANDLE) };
    if ok == 0 {
        let e = io::Error::last_os_error();
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Job Object 할당 실패: {e}"));
    }
    sh.pids.lock().unwrap().push(child.id());
    if let Err(e) = resume_process(child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    Ok(child)
}

fn run_one(
    sh: &Arc<Shared>,
    exe: &Path,
    args: &[String],
    tx: &Sender<FdEvent>,
    total: &mut u64,
    lossy: &mut u64,
) -> RunResult {
    // 취소와 spawn 이 겹치는 경우: 취소가 먼저면 Job 이 이미 종료돼 Assign 이 실패하거나
    // 곧바로 죽는다. 어느 쪽이든 이후 이벤트는 gate 가 막는다.
    let mut child = match spawn_in_job(sh, exe, args) {
        Ok(c) => c,
        Err(m) => {
            return if sh.is_cancelled() { RunResult::Aborted } else { RunResult::SpawnError(m) };
        }
    };
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    // stderr 수집 스레드: 줄 단위로 분류만 한다(이벤트는 코디네이터가 보낸다).
    let sh2 = sh.clone();
    let err_th = thread::spawn(move || {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 8192];
        let handle_line = |bytes: &[u8]| {
            let line = String::from_utf8_lossy(bytes);
            let line = line.trim_end_matches(['\r', '\n']).trim();
            if line.is_empty() {
                return;
            }
            let mut st = sh2.stderr.lock().unwrap();
            if st.head.len() < STDERR_OTHER_MAX {
                st.head.push(line.to_string());
            }
            if is_permission_line(line) {
                st.permission_errors += 1;
                st.dirty = true;
            } else if st.other.len() < STDERR_OTHER_MAX {
                st.other.push(line.to_string());
                st.dirty = true;
            }
        };
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    raw.extend_from_slice(&chunk[..n]);
                    while let Some(p) = raw.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = raw.drain(..=p).collect();
                        handle_line(&line);
                    }
                }
            }
        }
        if !raw.is_empty() {
            handle_line(&raw);
        }
    });

    // stdout 읽기 스레드: 블로킹 읽기를 코디네이터의 20ms flush 와 분리한다.
    let (ctx, crx) = mpsc::sync_channel::<Vec<u8>>(64);
    let out_th = thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ctx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut splitter = NulSplitter::new();
    let mut batch: Vec<String> = Vec::new();
    let mut last_flush = Instant::now();
    let mut last_err_emit = Instant::now();
    let mut aborted = false;

    let flush = |batch: &mut Vec<String>, total: &mut u64| -> bool {
        if batch.is_empty() {
            return true;
        }
        let n = batch.len() as u64;
        let v = std::mem::take(batch);
        let ok = sh.send(tx, FdEvent::Paths(v));
        if ok {
            *total += n;
        }
        ok
    };

    loop {
        match crx.recv_timeout(BATCH_INTERVAL) {
            Ok(chunk) => {
                splitter.push(&chunk, &mut batch);
                while batch.len() >= BATCH_MAX_ITEMS {
                    let rest = batch.split_off(BATCH_MAX_ITEMS);
                    if !flush(&mut batch, total) {
                        aborted = true;
                        break;
                    }
                    batch = rest;
                    last_flush = Instant::now();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if aborted {
            break;
        }
        if last_flush.elapsed() >= BATCH_INTERVAL {
            if !flush(&mut batch, total) {
                aborted = true;
                break;
            }
            last_flush = Instant::now();
        }
        if last_err_emit.elapsed() >= STDERR_EMIT_INTERVAL {
            if !emit_stderr_if_dirty(sh, tx) {
                aborted = true;
                break;
            }
            last_err_emit = Instant::now();
        }
    }

    if aborted {
        // 취소 또는 수신측 종료: Job 전체를 죽이고 정리한다.
        sh.kill_all();
        drop(crx);
        let _ = child.wait();
        let _ = out_th.join();
        let _ = err_th.join();
        return RunResult::Aborted;
    }

    splitter.finish(&mut batch);
    *lossy += splitter.lossy_count;
    if !flush(&mut batch, total) {
        sh.kill_all();
        let _ = child.wait();
        let _ = err_th.join();
        return RunResult::Aborted;
    }
    let _ = out_th.join();
    let status = child.wait();
    let _ = err_th.join();
    if sh.is_cancelled() {
        return RunResult::Aborted;
    }
    match status {
        Ok(s) if s.success() => RunResult::Ok,
        Ok(s) => RunResult::Failed(s.to_string()),
        Err(e) => RunResult::Failed(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// 테스트
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::mpsc::channel;

    fn split_all(chunks: &[&[u8]]) -> (Vec<String>, u64) {
        let mut s = NulSplitter::new();
        let mut out = Vec::new();
        for c in chunks {
            s.push(c, &mut out);
        }
        s.finish(&mut out);
        (out, s.lossy_count)
    }

    #[test]
    fn splitter_basic_and_chunk_boundary() {
        let (o, l) = split_all(&[b"ab\0cd\0"]);
        assert_eq!(o, vec!["ab", "cd"]);
        assert_eq!(l, 0);
        // 항목 중간, NUL 직전, NUL 직후에서 쪼갠다
        let (o, _) = split_all(&[b"a", b"b\0c", b"", b"d", b"\0", b"e\0"]);
        assert_eq!(o, vec!["ab", "cd", "e"]);
        // 바이트 단위로 쪼개도 같다
        let data = "가나\0한 글 [x]\0z\0".as_bytes();
        let chunks: Vec<&[u8]> = data.chunks(1).collect();
        let (o, _) = split_all(&chunks);
        assert_eq!(o, vec!["가나", "한 글 [x]", "z"]);
    }

    #[test]
    fn splitter_multibyte_split_across_chunks() {
        let b = "가".as_bytes(); // 3 bytes
        let (o, l) = split_all(&[&b[..1], &b[1..2], &b[2..], b"\0"]);
        assert_eq!(o, vec!["가"]);
        assert_eq!(l, 0);
    }

    #[test]
    fn splitter_newline_in_name() {
        let (o, _) = split_all(&[b"a\nb\0", b"c\r\nd\0"]);
        assert_eq!(o, vec!["a\nb", "c\r\nd"]);
    }

    #[test]
    fn splitter_empty_items_and_tail() {
        let (o, _) = split_all(&[b"\0\0a\0\0", b"\0b"]);
        assert_eq!(o, vec!["a", "b"]); // 빈 항목 버림, NUL 없는 꼬리 포함
        let (o, _) = split_all(&[b""]);
        assert!(o.is_empty());
    }

    #[test]
    fn splitter_invalid_utf8_counts() {
        let (o, l) = split_all(&[b"ok\0", b"bad\xff\xfe\0", b"x\xc3", b"\0tail\x80"]);
        assert_eq!(o.len(), 4);
        assert_eq!(o[0], "ok");
        assert!(o[1].contains('\u{FFFD}'));
        assert_eq!(l, 3); // bad, x\xc3(불완전), tail\x80
    }

    fn dirs(names: &[(&str, bool)]) -> Vec<DirItem> {
        names.iter().map(|(n, d)| DirItem { name: n.to_string(), is_dir: *d }).collect()
    }

    #[test]
    fn plan_excludes_case_insensitive() {
        let cfg = FdConfig::default();
        let drives = vec![PathBuf::from("C:\\"), PathBuf::from("D:\\")];
        let rd = |p: &Path| -> io::Result<Vec<DirItem>> {
            if p == Path::new("C:\\") {
                Ok(dirs(&[
                    ("Users", true),
                    ("WINDOWS", true),
                    ("program files", true),
                    ("Program Files (x86)", true),
                    ("$Recycle.Bin", true),
                    ("system volume information", true),
                    ("pagefile.sys", false),
                    ("Work", true),
                ]))
            } else {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "x"))
            }
        };
        let plan = compute_plan(&drives, &rd, &cfg.deny_top);
        assert_eq!(plan.top_dirs, vec![PathBuf::from("C:\\Users"), PathBuf::from("C:\\Work")]);
        // 못 읽는 D: 도 루트 depth-1 대상에는 남는다
        assert_eq!(plan.drive_roots, drives);
    }

    #[test]
    fn plan_runs_user_search_path_skips() {
        let cfg = FdConfig::default();
        let rd = |_: &Path| -> io::Result<Vec<DirItem>> { panic!("읽으면 안 됨") };
        for ua in [
            vec!["--search-path".to_string(), "X:\\a".to_string()],
            vec!["--search-path=X:\\a".to_string(), "pat".to_string()],
        ] {
            let runs = plan_runs(&cfg, &ua, true, &[PathBuf::from("C:\\")], &rd).unwrap();
            assert_eq!(runs.len(), 1);
            let r = &runs[0];
            assert_eq!(&r[..4], ["--absolute-path", "--color", "never", "--print0"]);
            assert!(r.contains(&"--hidden".to_string()));
            assert_eq!(&r[r.len() - ua.len()..], &ua[..]);
            assert_eq!(r.iter().filter(|a| a.starts_with("--search-path")).count(), 1);
        }
    }

    #[test]
    fn plan_runs_structure() {
        let cfg = FdConfig::default();
        let rd = |_: &Path| -> io::Result<Vec<DirItem>> { Ok(dirs(&[("A", true), ("Windows", true)])) };
        let ua = vec!["-e".to_string(), "txt".to_string()];
        let runs = plan_runs(&cfg, &ua, false, &[PathBuf::from("C:\\")], &rd).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(!runs[0].contains(&"--hidden".to_string())); // use_defaults=false
        assert!(runs[0].windows(2).any(|w| w == ["--search-path", "C:\\A"]));
        assert!(!runs[0].iter().any(|a| a == "C:\\Windows"));
        assert!(runs[1].windows(2).any(|w| w == ["--max-depth", "1"]));
        assert!(runs[1].windows(2).any(|w| w == ["--exclude", "Windows"]));
        assert!(runs[1].windows(2).any(|w| w == ["--search-path", "C:\\"]));
        // 사용자 인자는 맨 뒤
        assert_eq!(&runs[1][runs[1].len() - 2..], ["-e", "txt"]);
        // 사용자 깊이 지정 시 보충 검색 생략
        let runs = plan_runs(&cfg, &["-d".into(), "2".into()], true, &[PathBuf::from("C:\\")], &rd).unwrap();
        assert_eq!(runs.len(), 1);
        // 드라이브 없음
        assert!(plan_runs(&cfg, &[], true, &[], &rd).is_err());
    }

    #[test]
    fn plan_runs_chunking_long_cmdline() {
        let cfg = FdConfig::default();
        let rd = |_: &Path| -> io::Result<Vec<DirItem>> {
            Ok((0..2000).map(|i| DirItem { name: format!("folder_with_long_name_{i:05}"), is_dir: true }).collect())
        };
        let runs = plan_runs(&cfg, &[], true, &[PathBuf::from("C:\\")], &rd).unwrap();
        assert!(runs.len() > 2);
        let sp: usize = runs.iter().map(|r| r.iter().filter(|a| *a == "--search-path").count()).sum();
        assert_eq!(sp, 2000 + 1);
        for r in &runs {
            assert!(r.iter().map(|a| a.len() + 3).sum::<usize>() < 32_000);
        }
    }

    #[test]
    fn drives_enumerate_has_fixed() {
        let d = enumerate_drives(&DriveTypes::default());
        assert!(!d.is_empty());
        assert!(d.iter().any(|p| p == Path::new("C:\\")));
    }

    // ---- 통합 ----

    struct Outcome {
        paths: Vec<String>,
        events: Vec<FdEvent>,
    }

    fn collect(rx: &mpsc::Receiver<FdEvent>, timeout: Duration) -> Outcome {
        let mut paths = Vec::new();
        let mut events = Vec::new();
        let t0 = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ev) => {
                    if let FdEvent::Paths(p) = &ev {
                        paths.extend(p.iter().cloned());
                    }
                    let end = matches!(ev, FdEvent::Done { .. } | FdEvent::Failed(_));
                    events.push(ev);
                    if end {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            assert!(t0.elapsed() < timeout, "시간 초과");
        }
        Outcome { paths, events }
    }

    fn tmp_root(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fdpick_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        // fd 의 --absolute-path 결과와 비교하기 위해 정규화(8.3 이름 방지)
        let c = fs::canonicalize(&p).unwrap();
        let s = c.to_string_lossy().into_owned();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    }

    fn set_hidden(p: &Path) {
        let _ = Command::new("attrib").arg("+h").arg(p).creation_flags(CREATE_NO_WINDOW).status();
    }

    #[test]
    fn integration_tree() {
        let root = tmp_root("tree");
        let mut expected: BTreeSet<String> = BTreeSet::new();
        let mut add_file = |rel: &str| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            expected.insert(p.to_string_lossy().into_owned());
        };
        add_file("한글 폴더/파일 이름.txt");
        add_file("sp ace/[brackets] (1).txt");
        add_file("sp ace/a'b&c.txt");
        add_file(".hiddenfile");
        add_file(".dotdir/inner.txt");
        let deep = (0..30).map(|i| format!("d{i}")).collect::<Vec<_>>().join("/");
        add_file(&format!("{deep}/deep.txt"));
        add_file("top.txt");
        // 개행 파일명(Windows 에서는 생성 불가일 수 있음)
        let nl = root.join("new\nline.txt");
        let nl_ok = fs::write(&nl, b"x").is_ok();
        if nl_ok {
            expected.insert(nl.to_string_lossy().into_owned());
        } else {
            eprintln!("SKIP: 개행 파일명 생성 불가(Windows)");
        }
        // 속성 숨김
        let hid = root.join("attrhidden.txt");
        fs::write(&hid, b"x").unwrap();
        set_hidden(&hid);
        expected.insert(hid.to_string_lossy().into_owned());
        // 제외 대상은 use_defaults 에서만 빠진다
        let nm = root.join("node_modules/pkg/index.js");
        fs::create_dir_all(nm.parent().unwrap()).unwrap();
        fs::write(&nm, b"x").unwrap();
        let git = root.join(".git/config");
        fs::create_dir_all(git.parent().unwrap()).unwrap();
        fs::write(&git, b"x").unwrap();

        // fd 는 디렉터리도 출력하므로 파일만 받는다
        let run = |defaults: bool| -> BTreeSet<String> {
            let (tx, rx) = channel();
            let args = vec![
                "--type".to_string(),
                "f".to_string(),
                "--search-path".to_string(),
                root.to_string_lossy().into_owned(),
            ];
            let _job = start(&FdConfig::default(), args, defaults, tx).unwrap();
            let o = collect(&rx, Duration::from_secs(60));
            match o.events.last().unwrap() {
                FdEvent::Done { exit_ok, total, lossy_count, .. } => {
                    assert!(*exit_ok);
                    assert_eq!(*total as usize, o.paths.len());
                    assert_eq!(*lossy_count, 0);
                }
                other => panic!("Done 아님: {other:?}"),
            }
            let n = o.paths.len();
            let set: BTreeSet<String> = o.paths.into_iter().collect();
            assert_eq!(set.len(), n, "중복 항목");
            set
        };

        let with_def = run(true);
        let norm = |s: &BTreeSet<String>| -> BTreeSet<String> {
            s.iter().map(|p| p.replace('/', "\\")).collect()
        };
        let exp_norm: BTreeSet<String> = expected.iter().map(|p| p.replace('/', "\\")).collect();
        assert_eq!(norm(&with_def), exp_norm);
        assert!(!with_def.iter().any(|p| p.contains("node_modules") || p.contains(".git")));

        let no_def = run(false);
        let n = norm(&no_def);
        // 기본 옵션 없이는 --hidden 도 없어 숨김 항목이 빠지고, node_modules 는 보인다
        assert!(n.iter().any(|p| p.contains("node_modules")));
        assert!(!n.iter().any(|p| p.ends_with(".hiddenfile")));
        assert!(n.iter().any(|p| p.ends_with("deep.txt")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn integration_newline_name_via_nul() {
        // 개행 파일명 생성이 가능한 환경인지 확인하고, 가능하면 단일 항목으로 나오는지 본다.
        let root = tmp_root("nl");
        let nl = root.join("a\nb.txt");
        if fs::write(&nl, b"x").is_err() {
            eprintln!("SKIP: 개행 파일명 생성 불가(Windows) - NulSplitter 단위 테스트로 대체");
            let _ = fs::remove_dir_all(&root);
            return;
        }
        let (tx, rx) = channel();
        let args = vec!["--type".into(), "f".into(), "--search-path".into(), root.to_string_lossy().into_owned()];
        let _job = start(&FdConfig::default(), args, true, tx).unwrap();
        let o = collect(&rx, Duration::from_secs(30));
        assert_eq!(o.paths.len(), 1);
        assert!(o.paths[0].contains("a\nb.txt"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn integration_default_roots_drive_level() {
        // 사용자 search-path 없이: 드라이브 최상위 폴더 + 루트 파일을 모두 얻는지.
        // 전체 C:\ 를 돌리지 않도록 패턴으로 좁히고, 루트 파일/제외 폴더만 확인한다.
        let (tx, rx) = channel();
        let args = vec!["^pagefile\\.sys$|^Windows$|^Users$".to_string()];
        let _job = start(&FdConfig::default(), args, false, tx).unwrap();
        let o = collect(&rx, Duration::from_secs(120));
        assert!(matches!(o.events.last().unwrap(), FdEvent::Done { .. }), "{:?}", o.events.last());
        let lower: Vec<String> = o.paths.iter().map(|p| p.to_lowercase().trim_end_matches('\\').to_string()).collect();
        // 최상위 폴더 자체는 depth-1 보충 검색으로 나온다
        assert!(lower.iter().any(|p| p == "c:\\users"), "{lower:?}");
        // 제외 폴더는 depth-1 에서도 빠진다
        assert!(!lower.iter().any(|p| p == "c:\\windows"));
    }

    #[test]
    fn cancel_kills_tree_and_no_more_events() {
        let (tx, rx) = channel();
        let args = vec!["--search-path".to_string(), "C:\\Windows".to_string()];
        let job = start(&FdConfig::default(), args, false, tx).unwrap();
        // 결과가 흐르기 시작할 때까지 기다린다
        let t0 = Instant::now();
        let mut got = 0usize;
        while got < 1000 && t0.elapsed() < Duration::from_secs(30) {
            if let Ok(FdEvent::Paths(p)) = rx.recv_timeout(Duration::from_millis(100)) {
                got += p.len();
            }
        }
        assert!(got > 0, "결과가 오지 않음");
        let before = job.active_processes();
        assert!(before >= 1, "Job 안에 프로세스가 없음");
        let pids = job.child_pids();
        assert!(!pids.is_empty());

        let t = Instant::now();
        job.cancel();
        let took = t.elapsed();
        eprintln!("MEASURE: cancel() 소요 {took:?}, 취소 전 Job 내 프로세스 {before}개");
        assert_eq!(job.active_processes(), 0);
        assert!(job.is_finished());
        for pid in &pids {
            assert!(!pid_alive(*pid), "pid {pid} 가 살아 있음");
        }
        // 취소 이후 큐에 남은 것은 허용하되, cancel 반환 후 새 이벤트(특히 Done)는 없어야 한다.
        while rx.try_recv().is_ok() {}
        thread::sleep(Duration::from_millis(1500));
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Disconnected) | Err(mpsc::TryRecvError::Empty) => {}
            Ok(ev) => panic!("취소 후 이벤트 수신: {ev:?}"),
        }
        assert!(took < Duration::from_secs(3));
    }

    #[test]
    fn drop_kills_job() {
        let (tx, rx) = channel();
        let args = vec!["--search-path".to_string(), "C:\\Windows".to_string()];
        let job = start(&FdConfig::default(), args, false, tx).unwrap();
        let t0 = Instant::now();
        while job.child_pids().is_empty() && t0.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(10));
        }
        let _ = rx.recv_timeout(Duration::from_secs(5));
        let pids = job.child_pids();
        drop(job);
        thread::sleep(Duration::from_millis(300));
        for pid in pids {
            assert!(!pid_alive(pid));
        }
    }

    fn pid_alive(pid: u32) -> bool {
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == 259 // STILL_ACTIVE
        }
    }

    #[test]
    fn exe_not_found_is_failed() {
        let cfg = FdConfig { exe: Some(PathBuf::from("Z:\\nope\\fd.exe")), ..FdConfig::default() };
        let (tx, rx) = channel();
        let _job = start(&cfg, vec![], true, tx).unwrap();
        let o = collect(&rx, Duration::from_secs(10));
        match o.events.last().unwrap() {
            FdEvent::Failed(m) => {
                assert!(m.contains("fd"), "{m}");
                assert!(m.contains("nope"), "{m}");
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn bad_flag_is_failed_with_stderr() {
        let (tx, rx) = channel();
        let args = vec!["--no-such-flag".to_string(), "--search-path".to_string(), ".".to_string()];
        let _job = start(&FdConfig::default(), args, true, tx).unwrap();
        let o = collect(&rx, Duration::from_secs(10));
        match o.events.last().unwrap() {
            FdEvent::Failed(m) => {
                eprintln!("MSG: {m}");
                assert!(m.contains("no-such-flag"), "{m}");
            }
            e => panic!("{e:?}"),
        }
        assert!(o.paths.is_empty());
    }

    #[test]
    fn stderr_permission_classified() {
        assert!(is_permission_line("[fd error]: .\\x: Permission denied (os error 13)"));
        assert!(is_permission_line("[fd error]: C:\\System Volume Information: 액세스가 거부되었습니다. (os error 5)"));
        assert!(is_permission_line("[fd error]: Access is denied. (os error 5)"));
        assert!(!is_permission_line("[fd error]: something else"));
    }

    #[test]
    fn path_search_finds_fd() {
        // PATH 에 fd 가 있는 환경에서만 의미 있다
        if let Ok(p) = resolve_exe(&FdConfig::default()) {
            assert!(p.ends_with("fd.exe"));
        }
    }
}
