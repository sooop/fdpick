//! UI 컨트롤러(상태 머신). src/main.ts + src/list.ts 의 비-DOM 로직을 순수 Rust 로 포팅했다.
//! 창/스레드/파일 시스템에 접근하지 않고, 이벤트마다 셸이 수행할 `Command` 를 돌려준다.

use std::collections::HashMap;
use std::sync::Arc;

use fdpick_core::engine::{Mode, ResultItem, ResultsUpdated, RunState};
use fdpick_core::user_actions::ActionInfo;

use crate::pathutil::{
    ends_with_sep, ensure_trailing_sep, folder_of, normalize_copy_path, parent_input,
    points_at_folder, typed_filter,
};
use crate::view::{Command, KeyEvent, KeyOutcome, Row, StatusKind, ViewState};

pub struct ControllerInit {
    /// 마지막으로 쓴 모드(검색/경로). None 이면 경로 모드로 시작한다.
    pub saved_mode: Option<Mode>,
    /// 설정의 모든 사용자 액션(단축키 바인딩용)
    pub all_actions: Vec<ActionInfo>,
    /// 선택 대상에 적용 가능한 액션 조회(메뉴용). core::user_actions::applicable 를 감싼 클로저.
    pub applicable: Arc<dyn Fn(&[String]) -> Vec<ActionInfo> + Send + Sync>,
    /// 시작 알림(설정 경고, 단축키 등록 실패 등). 첫 입력 전까지 상태줄에 표시.
    pub startup_notes: Vec<String>,
}

const MENU_HELP: &str = "액션 메뉴 · 위/아래 선택 · Enter 실행 · Esc 닫기";

fn help_of(mode: Mode) -> &'static str {
    match mode {
        Mode::Fuzzy => "[fd옵션] -- 검색어 \u{00a0} 예) -e pdf -t f -- 보고서 · Tab 폴더 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
        Mode::Path => "폴더 탐색 · Tab 또는 / 진입 · Shift+Tab 상위 · Enter 복사 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
        Mode::Recent => "최근 사용 · 퍼지 검색 · Enter 복사 · Tab 폴더 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
    }
}

fn placeholder_of(mode: Mode) -> &'static str {
    match mode {
        Mode::Fuzzy => "[fd옵션] -- 검색어",
        Mode::Path => "경로 입력 (예: d:\\work)",
        Mode::Recent => "최근 사용 항목 검색",
    }
}

fn label_of(mode: Mode) -> &'static str {
    match mode {
        Mode::Fuzzy => "검색",
        Mode::Path => "경로",
        Mode::Recent => "최근",
    }
}

/// Ctrl+L 순환: fuzzy -> path -> recent -> fuzzy
fn next_mode(mode: Mode) -> Mode {
    match mode {
        Mode::Fuzzy => Mode::Path,
        Mode::Path => Mode::Recent,
        Mode::Recent => Mode::Fuzzy,
    }
}

/// 천 단위 구분(en-US)
fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 100ms 미만은 `NNms`, 이상은 `1.2s`(JS toFixed(1) 과 같은 반올림)
fn fmt_elapsed(ms: u64) -> String {
    if ms < 100 {
        return format!("{ms}ms");
    }
    // JS toFixed 는 정확한 이진값이 딱 중간일 때 올림, Rust 는 짝수 반올림이라 .25/.75 만 따로 처리
    if ms % 500 == 250 {
        let tenths = (ms + 50) / 100;
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    format!("{:.1}s", ms as f64 / 1000.0)
}

pub struct Controller {
    applicable: Arc<dyn Fn(&[String]) -> Vec<ActionInfo> + Send + Sync>,
    has_actions: bool,
    hotkeys: HashMap<String, ActionInfo>,

    mode: Mode,
    /// 마지막으로 저장한 모드(팝업 재표시 때 복원). TS 의 localStorage 에 해당.
    saved_mode: Mode,
    /// 컨트롤러가 아는 입력창 텍스트(on_key / on_input_changed / SetInput 으로 갱신)
    input: String,
    /// 직전에 SetInput 으로 내보낸 텍스트. 셸이 되돌려 보내는 EN_CHANGE 한 번을 무시하기 위함.
    echo: Option<String>,
    page_size: usize,

    // 목록(list.ts 의 VirtualList 상태)
    items: Vec<ResultItem>,
    cursor: isize,
    user_moved: bool,
    no_cursor: bool,
    picked: Vec<String>,
    empty_hint: Option<String>,

    last_seq: Option<u64>,
    last_payload: Option<ResultsUpdated>,
    status_override: Option<String>,
    startup_note: Option<String>,
    input_invalid: bool,

    // 액션 메뉴
    menu_open: bool,
    menu_actions: Vec<ActionInfo>,
    menu_targets: Vec<String>,
    /// 메뉴를 열기 직전의 (커서, 사용자 이동 여부)
    menu_saved: (isize, bool),
}

fn out(handled: bool, commands: Vec<Command>) -> KeyOutcome {
    KeyOutcome { handled, commands }
}

impl Controller {
    pub fn new(init: ControllerInit) -> Self {
        let saved_mode = if init.saved_mode == Some(Mode::Fuzzy) { Mode::Fuzzy } else { Mode::Path };
        let mut hotkeys = HashMap::new();
        for a in &init.all_actions {
            if !a.key.is_empty() {
                hotkeys.insert(a.key.clone(), a.clone());
            }
        }
        let startup_note =
            if init.startup_notes.is_empty() { None } else { Some(init.startup_notes.join(" · ")) };
        Self {
            applicable: init.applicable,
            has_actions: !init.all_actions.is_empty(),
            hotkeys,
            mode: saved_mode,
            saved_mode,
            input: String::new(),
            echo: None,
            page_size: 10,
            items: Vec::new(),
            cursor: 0,
            user_moved: false,
            no_cursor: false,
            picked: Vec::new(),
            empty_hint: None,
            last_seq: None,
            last_payload: None,
            status_override: None,
            startup_note,
            input_invalid: false,
            menu_open: false,
            menu_actions: Vec::new(),
            menu_targets: Vec::new(),
            menu_saved: (0, false),
        }
    }

    /// 목록 한 화면에 들어가는 행 수(PageUp/PageDown 이동량). 셸이 크기/DPI 가 바뀔 때 알린다.
    pub fn set_page_size(&mut self, n: usize) {
        self.page_size = n.max(1);
    }

    // ---------- 목록 조작(list.ts) ----------

    fn current(&self) -> Option<&ResultItem> {
        if self.cursor < 0 {
            return None;
        }
        self.items.get(self.cursor as usize)
    }

    fn current_is_recent(&self) -> bool {
        self.current().map(|i| i.recent).unwrap_or(false)
    }

    /// 경로 모드에서는 맨 위의 최근 항목을 건너뛴 첫 폴더 항목, 없으면 0.
    fn default_index(&self, items: &[ResultItem]) -> isize {
        if self.mode != Mode::Path {
            return 0;
        }
        items.iter().position(|it| !it.recent).unwrap_or(0) as isize
    }

    fn set_items(&mut self, items: Vec<ResultItem>) {
        let prev_path = self.current().map(|i| i.path.clone());
        let first = self.default_index(&items);
        let mut next = if self.no_cursor { -1 } else { first };
        if !self.no_cursor && self.user_moved {
            if let Some(pp) = prev_path {
                next = items.iter().position(|it| it.path == pp).map(|i| i as isize).unwrap_or(first);
            }
        }
        self.items = items;
        self.cursor = next;
    }

    fn clear_list(&mut self) {
        self.items.clear();
        self.cursor = 0;
        self.user_moved = false;
        self.no_cursor = false;
        self.empty_hint = None;
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        self.user_moved = true;
        let last = self.items.len() as isize - 1;
        if self.no_cursor {
            // 선택 없음에서 처음 움직이면 위쪽 이동은 마지막, 아래쪽 이동은 첫 항목으로
            self.no_cursor = false;
            self.cursor = if delta > 0 { 0 } else { last };
        } else {
            self.cursor = (self.cursor + delta).clamp(0, last);
        }
    }

    fn restore_cursor(&mut self, idx: isize, moved: bool) {
        self.user_moved = moved;
        if idx < 0 || self.items.is_empty() {
            self.no_cursor = idx < 0;
            self.cursor = if idx < 0 { -1 } else { 0 };
        } else {
            self.no_cursor = false;
            self.cursor = idx.min(self.items.len() as isize - 1);
        }
    }

    fn toggle_pick_and_advance(&mut self) {
        let Some(path) = self.current().map(|i| i.path.clone()) else {
            return;
        };
        if let Some(pos) = self.picked.iter().position(|p| *p == path) {
            self.picked.remove(pos);
        } else {
            self.picked.push(path);
        }
        self.move_cursor(1);
    }

    // ---------- 입력/모드 ----------

    /// 경로 모드에서 입력이 폴더를 가리키면 초기 선택을 두지 않는다.
    fn apply_cursor_policy(&mut self) {
        if self.mode == Mode::Path && points_at_folder(&self.input) {
            self.no_cursor = true;
            self.cursor = -1;
        } else {
            self.no_cursor = false;
        }
    }

    fn query(&self, force: bool) -> Command {
        Command::Query { text: self.input.clone(), mode: self.mode, force }
    }

    /// 입력을 바꾸고 즉시 질의한다(setInput).
    fn set_input(&mut self, text: &str) -> Vec<Command> {
        self.input = text.to_string();
        self.echo = Some(text.to_string());
        self.status_override = None;
        self.user_moved = false;
        self.apply_cursor_policy();
        vec![Command::SetInput(text.to_string()), self.query(false)]
    }

    fn save_mode_cmd(&mut self, cmds: &mut Vec<Command>) {
        if self.mode == Mode::Fuzzy || self.mode == Mode::Path {
            self.saved_mode = self.mode;
            cmds.push(Command::SaveMode(self.mode));
        }
    }

    fn switch_mode(&mut self, next: Mode) -> Vec<Command> {
        self.close_menu(true);
        self.input_invalid = false;
        self.mode = next;
        let mut cmds = Vec::new();
        self.save_mode_cmd(&mut cmds);
        self.input.clear();
        self.echo = Some(String::new());
        self.status_override = None;
        self.picked.clear();
        self.user_moved = false;
        // TS 와 다른 점: 이전 모드의 "선택 없음" 상태가 새 모드의 첫 결과까지 새는 것을 막는다.
        self.apply_cursor_policy();
        cmds.push(Command::SetInput(String::new()));
        cmds.push(self.query(false));
        cmds
    }

    /// 팝업이 표시될 때(재표시 포함): 입력/선택/메뉴를 초기화하고 질의 명령을 돌려준다.
    pub fn on_show(&mut self) -> Vec<Command> {
        self.close_menu(false);
        self.input_invalid = false;
        self.mode = self.saved_mode;
        self.input.clear();
        self.echo = Some(String::new());
        self.status_override = None;
        self.last_payload = None;
        self.picked.clear();
        self.clear_list();
        vec![Command::SetInput(String::new()), self.query(false)]
    }

    /// 입력창 텍스트가 바뀌었다(IME 조합 중 포함).
    pub fn on_input_changed(&mut self, text: &str, _composing: bool) -> Vec<Command> {
        let echoed = self.echo.take().as_deref() == Some(text);
        self.input = text.to_string();
        if echoed || self.menu_open {
            return Vec::new();
        }
        self.status_override = None;
        self.startup_note = None;
        self.user_moved = false;
        self.apply_cursor_policy();
        vec![self.query(false)]
    }

    // ---------- 입력 완성 ----------

    /// 커서 항목으로 입력을 완성하고 한 단계 진입(path 모드). 항목이 없으면 None.
    fn enter_cursor_dir(&mut self) -> Option<Vec<Command>> {
        let path = self.current()?.path.clone();
        Some(self.set_input(&ensure_trailing_sep(&path)))
    }

    /// 경로 모드에서 이름을 입력하던 중 Enter: 복사하지 않고 강조 항목으로 완성해 진입한다.
    fn complete_on_enter(&mut self) -> Option<Vec<Command>> {
        if self.mode != Mode::Path || !self.picked.is_empty() || self.user_moved {
            return None;
        }
        let it = self.current()?;
        if it.recent {
            return None;
        }
        let typed = typed_filter(&self.input);
        if typed.is_empty() {
            return None;
        }
        let name: String = it.path.chars().skip(it.name_start).collect();
        if name.to_lowercase() == typed.to_lowercase() {
            return None;
        }
        self.enter_cursor_dir()
    }

    /// Shift+Tab: 경로 모드에서 한 단계 상위로
    fn go_back(&mut self) -> Vec<Command> {
        if self.mode == Mode::Path && !self.input.is_empty() {
            let p = parent_input(&self.input);
            return self.set_input(&p);
        }
        Vec::new()
    }

    /// fuzzy/recent 모드 Tab, 경로 모드의 최근 항목 진입: 커서 항목의 폴더로 경로 모드 시작
    fn fuzzy_to_path(&mut self) -> Vec<Command> {
        let Some(it) = self.current() else {
            return Vec::new();
        };
        let dir = ensure_trailing_sep(&folder_of(&it.path, it.is_dir));
        let mut cmds = Vec::new();
        if self.mode != Mode::Path {
            self.mode = Mode::Path;
            self.save_mode_cmd(&mut cmds);
        }
        self.picked.clear();
        cmds.extend(self.set_input(&dir));
        cmds
    }

    // ---------- 액션 ----------

    fn action_targets(&self) -> Vec<String> {
        if !self.picked.is_empty() {
            return self.picked.clone();
        }
        if let Some(it) = self.current() {
            return vec![it.path.clone()];
        }
        if self.mode == Mode::Path {
            let p = normalize_copy_path(&self.input);
            if !p.is_empty() {
                return vec![p];
            }
        }
        Vec::new()
    }

    fn run_action(&self, kind: &str) -> Vec<Command> {
        let paths = self.action_targets();
        if paths.is_empty() {
            return Vec::new();
        }
        vec![Command::RunAction { kind: kind.to_string(), paths }]
    }

    fn run_user(act: &ActionInfo, paths: Vec<String>) -> Command {
        Command::RunUserAction { id: act.id, paths, close_popup: act.close_popup }
    }

    /// RunAction / RunUserAction 완료 통지. 성공이면 close_popup 일 때 Hide, 실패면 상태줄에 오류.
    pub fn on_action_result(&mut self, r: Result<(), String>, close_popup: bool) -> Vec<Command> {
        match r {
            Ok(()) => {
                if close_popup {
                    vec![Command::Hide]
                } else {
                    Vec::new()
                }
            }
            Err(e) => {
                self.status_override = Some(e);
                Vec::new()
            }
        }
    }

    fn open_menu(&mut self) {
        let paths = self.action_targets();
        if paths.is_empty() {
            return;
        }
        let acts = (self.applicable)(&paths);
        if self.menu_open {
            return;
        }
        if acts.is_empty() {
            self.status_override = Some("적용할 수 있는 액션이 없습니다".to_string());
            return;
        }
        self.menu_saved = (self.cursor, self.user_moved);
        let items: Vec<ResultItem> = acts
            .iter()
            .map(|a| ResultItem {
                path: a.name.clone(),
                name_start: 0,
                indices: Vec::new(),
                is_dir: false,
                recent: false,
                missing: false,
            })
            .collect();
        self.menu_actions = acts;
        self.menu_targets = paths;
        self.menu_open = true;
        self.no_cursor = false;
        self.set_items(items);
        self.restore_cursor(0, false);
        self.empty_hint = None;
    }

    /// 메뉴 닫기. restore 가 true 면 이전 목록/커서를 복원한다.
    fn close_menu(&mut self, restore: bool) {
        if !self.menu_open {
            return;
        }
        self.menu_open = false;
        self.menu_actions.clear();
        self.menu_targets.clear();
        if restore {
            if let Some(p) = self.last_payload.clone() {
                self.apply_payload(p);
            } else {
                self.clear_list();
            }
            let (idx, moved) = self.menu_saved;
            self.restore_cursor(idx, moved);
        }
    }

    // ---------- 결과 ----------

    fn update_invalid(&mut self, p: &ResultsUpdated) {
        self.input_invalid = self.mode == Mode::Path
            && !self.input.trim().is_empty()
            && (p.state == RunState::Error
                || (!typed_filter(&self.input).is_empty() && !p.items.iter().any(|it| !it.recent)));
    }

    fn apply_payload(&mut self, p: ResultsUpdated) {
        self.status_override = None;
        self.update_invalid(&p);
        self.set_items(p.items.clone());
        let text = if self.mode == Mode::Recent && self.input.trim().is_empty() {
            "최근 사용 기록이 없습니다"
        } else {
            "일치하는 항목이 없습니다"
        };
        let show = p.items.is_empty() && (p.state == RunState::Done || p.state == RunState::Idle);
        self.empty_hint = if show { Some(text.to_string()) } else { None };
        self.last_payload = Some(p);
    }

    /// 엔진 결과(어느 스레드에서 왔든 셸이 UI 스레드에서 호출). 오래된 seq / 다른 모드는 버린다.
    pub fn on_results(&mut self, p: ResultsUpdated) {
        if self.last_seq.is_some_and(|s| p.seq <= s) || p.mode != self.mode {
            return;
        }
        self.last_seq = Some(p.seq);
        if self.menu_open {
            // 메뉴 중에는 목록을 덮어쓰지 않고 닫을 때 반영한다
            self.last_payload = Some(p);
            return;
        }
        self.apply_payload(p);
    }

    /// 목록 행 클릭(커서 이동).
    pub fn on_row_click(&mut self, row: usize) {
        if row < self.items.len() {
            self.user_moved = true;
            self.no_cursor = false;
            self.cursor = row as isize;
        }
    }

    /// 창이 포커스를 잃음. 컨트롤러는 조합 상태를 직접 추적하지 않으므로(KeyEvent.composing 사용) 할 일이 없다.
    pub fn on_blur(&mut self) {}

    // ---------- 키 ----------

    /// 눌린 키 조합을 설정 파일과 같은 정규화 형식("Ctrl+Alt+Shift+키")으로 만든다.
    fn combo_of(ev: &KeyEvent) -> String {
        let mut parts: Vec<String> = Vec::new();
        if ev.ctrl {
            parts.push("Ctrl".into());
        }
        if ev.alt {
            parts.push("Alt".into());
        }
        if ev.shift {
            parts.push("Shift".into());
        }
        let k = if ev.key == " " {
            "Space".to_string()
        } else if ev.key.chars().count() == 1 {
            ev.key.to_uppercase()
        } else {
            ev.key.clone()
        };
        parts.push(k);
        parts.join("+")
    }

    /// 키 입력. `input_text` 는 현재 입력창 텍스트.
    pub fn on_key(&mut self, ev: &KeyEvent, input_text: &str) -> KeyOutcome {
        if self.echo.as_deref() != Some(input_text) {
            self.echo = None;
        }
        self.input = input_text.to_string();
        // 조합 중에는 명령 키를 처리하지 않는다(IME 확정용 키)
        if ev.composing {
            return out(false, Vec::new());
        }
        if self.menu_open {
            return self.on_menu_key(ev);
        }

        let key = ev.key.as_str();
        let ctrl = ev.ctrl && !ev.alt;
        let lower = if key.chars().count() == 1 { key.to_lowercase() } else { key.to_string() };
        let lower = lower.as_str();

        if ctrl && lower == "k" {
            self.open_menu();
            return out(true, Vec::new());
        }

        if key == "Tab" {
            if ev.ctrl || ev.alt {
                return out(true, Vec::new());
            }
            if ev.shift {
                return out(true, self.go_back());
            }
            let cmds = if self.mode == Mode::Path && !self.current_is_recent() {
                self.enter_cursor_dir().unwrap_or_default()
            } else {
                self.fuzzy_to_path()
            };
            return out(true, cmds);
        }

        if key == "Escape" {
            return out(true, vec![Command::Hide]);
        }

        if key == "Enter" {
            let cmds = if ev.alt && !ev.ctrl {
                self.run_action("open")
            } else if ev.ctrl && !ev.alt && !ev.shift {
                self.run_action("reveal")
            } else if !ev.ctrl && !ev.shift {
                match self.complete_on_enter() {
                    Some(c) => c,
                    None => self.run_action("copy"),
                }
            } else {
                Vec::new()
            };
            return out(true, cmds);
        }

        if key == "ArrowUp" || (ctrl && lower == "p") {
            self.move_cursor(-1);
            return out(true, Vec::new());
        }
        if key == "ArrowDown" || (ctrl && lower == "n") {
            self.move_cursor(1);
            return out(true, Vec::new());
        }
        if key == "PageUp" {
            self.move_cursor(-(self.page_size as isize));
            return out(true, Vec::new());
        }
        if key == "PageDown" {
            self.move_cursor(self.page_size as isize);
            return out(true, Vec::new());
        }

        if ctrl && (key == "Space" || key == " ") {
            self.toggle_pick_and_advance();
            return out(true, Vec::new());
        }
        if ctrl && lower == "l" {
            let next = next_mode(self.mode);
            return out(true, self.switch_mode(next));
        }
        if ctrl && lower == "d" {
            return out(true, self.run_action("copy_dir"));
        }
        if ctrl && lower == "o" {
            return out(true, self.run_action("reveal"));
        }
        if ctrl && lower == "r" {
            return out(true, vec![self.query(true)]);
        }

        // 사용자 액션 단축키
        if !self.hotkeys.is_empty() {
            if let Some(act) = self.hotkeys.get(&Self::combo_of(ev)).cloned() {
                let paths = self.action_targets();
                let cmds =
                    if paths.is_empty() { Vec::new() } else { vec![Self::run_user(&act, paths)] };
                return out(true, cmds);
            }
        }

        // 경로 모드의 구분자: 입력 끝에 캐럿이 있을 때만 완성/진입으로 처리한다.
        if self.mode == Mode::Path && (key == "/" || key == "\\") && !ev.ctrl && !ev.alt {
            if !ev.caret_at_end {
                return out(false, Vec::new());
            }
            // 구분자 키는 섣불리 완성하지 않는다. 입력이 비었으면 아무것도 하지 않고,
            // 최근 항목은 사용자가 직접 골랐을 때만 진입하며, 그 외에는 구분자만 붙인다.
            if self.input.trim().is_empty() {
                return out(true, Vec::new());
            }
            let cmds = if self.current_is_recent() {
                if self.user_moved {
                    self.fuzzy_to_path()
                } else {
                    self.insert_sep()
                }
            } else {
                match self.enter_cursor_dir() {
                    Some(c) => c,
                    None => self.insert_sep(),
                }
            };
            return out(true, cmds);
        }

        out(false, Vec::new())
    }

    fn insert_sep(&mut self) -> Vec<Command> {
        if !ends_with_sep(&self.input) {
            let t = format!("{}\\", self.input);
            return self.set_input(&t);
        }
        Vec::new()
    }

    /// 메뉴가 열려 있는 동안의 키 처리
    fn on_menu_key(&mut self, ev: &KeyEvent) -> KeyOutcome {
        let key = ev.key.as_str();
        let ctrl = ev.ctrl && !ev.alt;
        let lower = if key.chars().count() == 1 { key.to_lowercase() } else { key.to_string() };
        let lower = lower.as_str();
        if key == "Escape" {
            self.close_menu(true);
            return out(true, Vec::new());
        }
        if key == "Enter" {
            let mut cmds = Vec::new();
            if self.cursor >= 0 && !ev.ctrl && !ev.alt && !ev.shift {
                if let Some(act) = self.menu_actions.get(self.cursor as usize) {
                    cmds.push(Self::run_user(act, self.menu_targets.clone()));
                }
            }
            return out(true, cmds);
        }
        if key == "ArrowUp" || (ctrl && lower == "p") {
            self.move_cursor(-1);
            return out(true, Vec::new());
        }
        if key == "ArrowDown" || (ctrl && lower == "n") {
            self.move_cursor(1);
            return out(true, Vec::new());
        }
        if key == "PageUp" {
            self.move_cursor(-(self.page_size as isize));
            return out(true, Vec::new());
        }
        if key == "PageDown" {
            self.move_cursor(self.page_size as isize);
            return out(true, Vec::new());
        }
        // 메뉴 중 내장 명령은 무시(Tab 의 포커스 이동도 막는다)
        let ignored = key == "Tab"
            || key == "/"
            || key == "\\"
            || (ctrl && (key == "Space" || key == " " || ["d", "o", "l", "r", "k"].contains(&lower)));
        out(ignored, Vec::new())
    }

    // ---------- 화면 ----------

    fn render_status(&self) -> (String, StatusKind) {
        let mut kind = StatusKind::Normal;
        let mut parts: Vec<String> = Vec::new();
        if self.menu_open {
            parts.push("액션 메뉴".into());
        }
        if !self.picked.is_empty() {
            parts.push(format!("{}개 선택", fmt_num(self.picked.len() as u64)));
        }
        if let Some(n) = &self.startup_note {
            kind = StatusKind::Error;
            parts.push(n.clone());
        }
        if let Some(o) = &self.status_override {
            kind = StatusKind::Error;
            parts.push(o.clone());
        } else if let Some(p) = &self.last_payload {
            let t = fmt_elapsed(p.elapsed_ms);
            if p.state == RunState::Error {
                kind = StatusKind::Error;
                parts.push(p.message.clone().unwrap_or_else(|| "오류".into()));
            } else if self.mode == Mode::Recent {
                parts.push(format!("{}개 일치", fmt_num(p.matched as u64)));
            } else if p.state == RunState::Running {
                if kind == StatusKind::Normal {
                    kind = StatusKind::Running;
                }
                parts.push(format!(
                    "검색 중… {}개 스캔 · {}개 일치 · {}",
                    fmt_num(p.scanned as u64),
                    fmt_num(p.matched as u64),
                    t
                ));
            } else if p.state == RunState::Done {
                parts.push(format!(
                    "완료 · {}개 일치 / {}개 스캔 · {}",
                    fmt_num(p.matched as u64),
                    fmt_num(p.scanned as u64),
                    t
                ));
            } else {
                parts.push("준비됨".into());
            }
            if p.state != RunState::Error {
                if let Some(m) = p.message.as_ref().filter(|m| !m.is_empty()) {
                    parts.push(m.clone());
                }
            }
            if let Some(c) = p.cache_note.as_ref().filter(|c| !c.is_empty()) {
                parts.push(c.clone());
            }
        } else {
            parts.push("준비됨".into());
        }
        (parts.join(" · "), kind)
    }

    fn rows(&self) -> Vec<Row> {
        let badge_mode = self.mode == Mode::Path && !self.menu_open;
        self.items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let recent_badge = badge_mode && it.recent;
                let next_recent = self.items.get(i + 1).map(|n| n.recent).unwrap_or(false);
                let hint = if self.menu_open {
                    self.menu_actions.get(i).map(|a| a.key.clone()).filter(|k| !k.is_empty())
                } else {
                    None
                };
                Row {
                    path: it.path.clone(),
                    name_start: it.name_start,
                    indices: it.indices.clone(),
                    is_dir: it.is_dir,
                    recent_badge,
                    missing: it.missing,
                    recent_last: recent_badge && !next_recent,
                    picked: !self.menu_open && self.picked.contains(&it.path),
                    hint,
                }
            })
            .collect()
    }

    pub fn view(&self) -> ViewState {
        let (status, status_kind) = self.render_status();
        let help = if self.menu_open {
            MENU_HELP.to_string()
        } else {
            format!("{}{}", help_of(self.mode), if self.has_actions { " · Ctrl+K 액션" } else { "" })
        };
        ViewState {
            mode: self.mode,
            mode_label: label_of(self.mode).to_string(),
            help,
            placeholder: placeholder_of(self.mode).to_string(),
            rows: self.rows(),
            cursor: if self.cursor >= 0 && (self.cursor as usize) < self.items.len() {
                Some(self.cursor as usize)
            } else {
                None
            },
            status,
            status_kind,
            input_invalid: self.input_invalid,
            empty_hint: self.empty_hint.clone(),
            menu_open: self.menu_open,
        }
    }
}
