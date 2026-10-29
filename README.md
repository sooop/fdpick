# fdpick

Windows 용 Spotlight 스타일 경로 검색기. 인덱스 없이 `fd` 로 파일시스템을 훑고 내장 fuzzy 매처(nucleo)로 거른 뒤,
선택한 경로를 클립보드에 복사하거나 탐색기로 연다. Rust + Win32 네이티브 UI(WebView 없음).

실측(릴리스, 단일 프로세스, 엔진 포함): exe 약 1MB, 숨김 유휴 개인 메모리 약 3MB, 표시 약 4MB.
(이전 Tauri 버전은 유휴 약 126MB. 태그 `tauri-final` 에 남아 있다.)

## 개발 환경

- Rust(`stable-x86_64-pc-windows-gnu`) + MSYS2 mingw64(`windres`, `gcc` 가 PATH 에 있어야 함, 아이콘 리소스 컴파일용). MSVC 불필요.
- `fd`(PATH 또는 설정의 `fd_path`).
- 글꼴: `Jetendard` 를 쓴다(없으면 대체 글꼴). 웹폰트 없음.

## 빌드와 실행

```
cargo build --release -p fdpick     # target\release\fdpick.exe
cargo test --workspace
```

실행하면 숨김 상태로 트레이에 상주한다. 기본 단축키 `Ctrl+Alt+Space` 는 검색 모드로, 2차 `Ctrl+Shift+L` 은 경로 모드로, 3차 `Ctrl+Alt+\` 는 앱 모드로 열고 닫는다. `--show` 를 주면 시작과 동시에 표시한다.
단축키가 다른 앱과 겹치면 환경변수 `FDPICK_HOTKEY="Ctrl+Alt+F"` 로 1차를 임시 지정하거나 설정 파일의 `hotkey`·`hotkey2` 를 바꾼다. 2차는 `hotkey2 = ""` 로 끌 수 있다.
등록에 실패해도 앱은 트레이로 계속 실행된다.

## 모드와 키

| 모드 | 용도 |
|---|---|
| 검색 | `[fd옵션] -- 검색어` 로 fd 결과를 fuzzy 검색 |
| 경로 | 폴더를 한 단계씩 내려가며 경로를 만든다(입력이 비면 드라이브 목록, 입력 중이면 최근 항목 5개를 위에 표시) |
| 최근 | 복사/열기한 경로의 최근 사용 목록(fuzzy 검색) |
| 앱 | 시작 메뉴 앱·스토어 앱·윈도 내장 도구(장치 관리자, 제어판 등)·설정 페이지를 실행. Ctrl+L 순환의 마지막 모드이며 3차 단축키로 바로 열 수도 있음 |

| 키 | 동작 |
|---|---|
| Ctrl+L | 모드 순환(검색, 경로, 최근) |
| Enter | `enter_action` 동작(기본 `to_explorer`). 경로 모드에서 이름 입력 중이면 강조 항목으로 완성(한 번 더 누르면 실행) |
| Ctrl+D | 폴더 경로 복사 |
| Ctrl+O, Ctrl+Enter | 탐색기에서 열기 |
| Shift+Enter | 복사(`shift_enter_action`, 기본 `copy`). `to_explorer` 로 지정하면 팝업 직전에 포커스가 있던 탐색기 창(활성 탭)을 그 경로로 이동. 파일이면 부모 폴더로 이동해 선택. 직전 포커스가 탐색기가 아니면(다른 탐색기가 열려 있어도) `reveal` 처럼 탐색기를 새로 실행하고, 탐색기 창 목록을 읽지 못하거나 이동에 실패해도 같은 방식으로 대신함 |
| Alt+Enter | 기본 프로그램으로 열기 |
| Tab, `/` | 강조 폴더로 진입 |
| Shift+Tab | 경로 모드에서 상위 폴더 |
| Ctrl+Space | 다중 선택 토글 |
| Ctrl+R | fd 다시 실행 |
| Ctrl+K, →(캐럿이 입력 끝일 때) | 사용자 액션 메뉴(메뉴 안에서 ←, Esc 로 닫기) |
| Ctrl+↑/↓ | 검색 모드: 검색 패턴 이력 이전/다음 |
| Esc | 닫기 |

경로 모드에서 입력 맨 앞의 `~`, 검색 모드에서는 입력 끝에서 치는 `@~` 토큰의 `~`(단독 또는 `~\`, `~/` 로 시작)는 사용자 프로파일 폴더(`%USERPROFILE%`)로 치환된다.
`enter_action`·`shift_enter_action` 값: `copy`, `copy_dir`, `reveal`(탐색기에서 열기), `open`, `to_explorer`.
설정 파일을 고친 뒤에는 트레이 메뉴의 "다시 시작 (설정 다시 읽기)" 로 반영한다.

상세 사양은 `docs/path-mode.md`.

## 설정

`%APPDATA%\fdpick\config.toml`(없으면 첫 실행에 생성). 단축키, fd 경로와 기본 옵션, 제외 폴더, 캐시 시간, 결과 개수,
최근 목록 개수, 포커스 아웃 숨김, 테마, 자동 시작, 복사 형식, `[[action]]` 사용자 액션을 설정한다.
최근 목록은 `%APPDATA%\fdpick\recent.json`, 검색 패턴 이력은 `%APPDATA%\fdpick\search_history.json`.

검색 패턴 이력: 검색 모드에서 결과를 실제로 쓴(Enter, 복사, 열기, 액션 실행) 입력을 최대 `history_limit`(기본 50, 0 이면 끔)개 저장한다.
같은 입력은 하나로 합쳐 맨 앞에 둔다. 검색 모드에서 Ctrl+↑/↓ 로 이전/다음 패턴을 입력창에 채우며(입력 중이면 그 글자로 시작하는 패턴만),
Ctrl+↓ 로 끝까지 내려오면 처음 입력으로 돌아온다. 전체 삭제는 트레이 메뉴의 "검색 이력 지우기".

## 구조

- `core/` (`fdpick-core`): UI 와 무관한 엔진. `fd`(실행, 스트리밍, 취소), `matcher`(nucleo), `pathnav`(경로 탐색),
  `recents`, `config`, `actions`(복사, 열기, 경로 정규화), `user_actions`, `engine`(질의 조율, 결과는 콜백으로 전달).
- `logic/` (`fdpick-logic`): 모드, 키, 선택, 상태줄을 다루는 순수 상태 머신(`Controller`). 창 시스템과 무관해 단위 테스트가 가능하다.
- `app/` (`fdpick`): Win32 셸. 창, 시스템 EDIT 입력(한글 IME), GDI 더블버퍼 목록, 트레이, 전역 단축키, 단일 인스턴스, 자동 시작.
- `spike/`: UI 후보(Win32, Slint) 프로토타입과 메모리 측정 스크립트(`measure.ps1`). 빌드에는 쓰이지 않는다.
- `reference/`: 초기 PowerShell+AutoHotkey 프로토타입(읽기 전용 참고).

## 앱 모드

`hotkey3`(기본 `Ctrl+Alt+\`)로 열거나 Ctrl+L 순환으로 들어간다. 문서·파일은 검색하지 않고 실행할 항목만 다룬다.

- 항목: 셸 AppsFolder(시작 메뉴 앱, 스토어 앱, 현재 로캘 이름) + 내장 목록(`.msc`/`.cpl` 도구, `ms-settings:` 설정 페이지).
  앱 모드를 처음 열거나 30분이 지나 다시 열면 백그라운드로 목록을 다시 모은다(열거 약 0.3초). Ctrl+R 로 즉시 새로고침.
- 검색: 공백 무시("장치관리자" = "장치 관리자"), 대소문자 무시, 한글 초성(`ㅈㅊㄱㄹㅈ`), 초성과 글자 혼용(`장ㅊㄱ`), 영문 약어(`wt`), 내장 도구의 영문 별칭(`devmgmt`).
- 목록 위쪽에 최근 실행한 앱 5개까지("최근" 배지, 강조색, 구분선), 아래에 전체. 최근 앱도 전체에 다시 나온다. 실행 시각은 `%APPDATA%\fdpick\apps_recent.json` 에 저장한다.
- Enter 로 실행, Shift+Enter 로 관리자 권한 실행(UAC 확인은 직접 처리). 실행 파일이 확인되는 앱과 내장 도구만 지원하고 스토어 앱·설정 페이지는 지원하지 않는다.
- 제거 프로그램, 웹 링크, 문서·스크립트 파일(`.html`, `.chm`, `.ahk` 등)은 목록에서 뺀다. 이 PC에 없는 내장 도구(Home 에디션의 `gpedit.msc` 등)도 뺀다.
- 경로가 아니므로 복사·탐색기·사용자 액션 키는 이 모드에서 동작하지 않는다.
