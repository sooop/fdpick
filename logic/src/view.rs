//! UI 계약: 컨트롤러(순수 로직)와 Win32 셸이 주고받는 타입.
//!
//! 흐름: 셸이 창 이벤트를 `Controller` 의 `on_*` 메서드에 넘기면, 컨트롤러는 내부 상태를 바꾸고
//! 셸이 수행해야 할 `Command` 목록을 돌려준다. 셸은 `Controller::view()` 의 `ViewState` 로 화면을 그린다.
//! 컨트롤러는 스레드, 창, 파일 시스템에 직접 접근하지 않는다(적용 가능 액션 조회만 주입된 클로저로 한다).

use fdpick_core::engine::Mode;

/// 목록 한 행의 표시 정보. 문자 위치는 모두 char(유니코드 코드포인트) 인덱스다.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// 표시할 전체 경로(액션 메뉴에서는 액션 이름)
    pub path: String,
    /// path 안에서 파일명이 시작하는 char 인덱스(이 앞이 상위 경로)
    pub name_start: usize,
    /// 매치된 글자의 char 인덱스(오름차순)
    pub indices: Vec<u32>,
    pub is_dir: bool,
    /// "최근" 배지를 붙인다(경로 모드의 최근 구역)
    pub recent_badge: bool,
    /// 존재하지 않는 경로(흐리게)
    pub missing: bool,
    /// 최근 구역의 마지막 행(아래에 구분선)
    pub recent_last: bool,
    /// Ctrl+Space 로 다중 선택됨
    pub picked: bool,
    /// 행 오른쪽의 보조 문구(액션 메뉴의 단축키)
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Normal,
    Running,
    Error,
}

/// 화면 전체 상태. 셸은 이것만 보고 그린다.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub mode: Mode,
    /// 모드 칩 문구: "검색" / "경로" / "최근"
    pub mode_label: String,
    /// 하단 도움말(왼쪽)
    pub help: String,
    /// 입력창 placeholder
    pub placeholder: String,
    pub rows: Vec<Row>,
    /// 선택된 행. None 이면 선택 없음(경로 모드에서 폴더에 막 진입한 상태 등)
    pub cursor: Option<usize>,
    /// 하단 상태줄(오른쪽)
    pub status: String,
    pub status_kind: StatusKind,
    /// 입력 글자를 빨갛게(존재하지 않는 경로)
    pub input_invalid: bool,
    /// 목록이 비었을 때 가운데에 보일 안내 문구
    pub empty_hint: Option<String>,
    /// 액션 메뉴가 열려 있음
    pub menu_open: bool,
}

/// 셸이 번역해 넘기는 키 입력. `key` 는 웹의 KeyboardEvent.key 와 같은 이름을 쓴다:
/// "Enter" "Tab" "Escape" "ArrowUp" "ArrowDown" "PageUp" "PageDown" "Home" "End" "Delete" "Insert"
/// "Backspace" "Space" "F1".."F24", 그 외 한 글자는 그 글자(영문은 대문자, "/" "\\" 등은 그대로).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// 입력창 캐럿이 텍스트 끝에 있고 선택 영역이 없음
    pub caret_at_end: bool,
    /// IME 조합 중(조합 중에는 명령 키를 처리하지 않는다)
    pub composing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOutcome {
    /// true 면 셸은 이 키를 입력창(EDIT)에 전달하지 않는다(기본 동작 차단)
    pub handled: bool,
    pub commands: Vec<Command>,
}

/// 컨트롤러가 셸에 요청하는 동작
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// 입력창 텍스트를 교체하고 캐럿을 끝으로 보낸다(EN_CHANGE 가 다시 오면 컨트롤러가 무시하도록
    /// 컨트롤러는 같은 텍스트의 on_input_changed 를 중복 처리해도 안전해야 한다)
    SetInput(String),
    /// 엔진에 질의: Engine::query_changed(text, mode, force)
    Query { text: String, mode: Mode, force: bool },
    /// 팝업 숨김
    Hide,
    /// 내장 동작(core::actions::run_with): kind 는 "copy" "copy_dir" "reveal" "open" "to_explorer"
    /// 완료되면 셸이 `Controller::on_action_result` 를 호출한다.
    RunAction { kind: String, paths: Vec<String> },
    /// 사용자 액션(core::user_actions::run). 완료되면 `on_action_result` 호출.
    RunUserAction { id: usize, paths: Vec<String>, close_popup: bool },
    /// 검색 패턴 이력 전체(최신이 앞)를 저장한다. 빈 목록이면 이력을 지운 것.
    SaveHistory(Vec<String>),
}
