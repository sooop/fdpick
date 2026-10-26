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

실행하면 숨김 상태로 트레이에 상주한다. 기본 단축키 `Ctrl+Alt+Space`(2차 `Ctrl+Alt+L`)로 열고 닫는다. `--show` 를 주면 시작과 동시에 표시한다.
단축키가 다른 앱과 겹치면 환경변수 `FDPICK_HOTKEY="Ctrl+Alt+F"` 로 1차를 임시 지정하거나 설정 파일의 `hotkey`·`hotkey2` 를 바꾼다. 2차는 `hotkey2 = ""` 로 끌 수 있다.
등록에 실패해도 앱은 트레이로 계속 실행된다.

## 모드와 키

| 모드 | 용도 |
|---|---|
| 검색 | `[fd옵션] -- 검색어` 로 fd 결과를 fuzzy 검색 |
| 경로 | 폴더를 한 단계씩 내려가며 경로를 만든다(입력이 비면 드라이브 목록, 입력 중이면 최근 항목 5개를 위에 표시) |
| 최근 | 복사/열기한 경로의 최근 사용 목록(fuzzy 검색) |

| 키 | 동작 |
|---|---|
| Ctrl+L | 모드 순환(검색, 경로, 최근) |
| Enter | 복사. 경로 모드에서 이름 입력 중이면 강조 항목으로 완성(한 번 더 누르면 복사) |
| Ctrl+D | 폴더 경로 복사 |
| Ctrl+O, Ctrl+Enter | 탐색기에서 열기 |
| Shift+Enter | 팝업 직전에 포커스가 있던 탐색기 창(활성 탭)을 그 경로로 이동. 파일이면 부모 폴더로 이동해 선택. 탐색기 창이 하나도 없으면 새 창을 열고, 다른 탐색기만 열려 있고 직전 포커스가 탐색기가 아니면 아무 일도 하지 않음 |
| Alt+Enter | 기본 프로그램으로 열기 |
| Tab, `/` | 강조 폴더로 진입 |
| Shift+Tab | 경로 모드에서 상위 폴더 |
| Ctrl+Space | 다중 선택 토글 |
| Ctrl+R | fd 다시 실행 |
| Ctrl+K | 사용자 액션 메뉴 |
| Esc | 닫기 |

상세 사양은 `docs/path-mode.md`.

## 설정

`%APPDATA%\fdpick\config.toml`(없으면 첫 실행에 생성). 단축키, fd 경로와 기본 옵션, 제외 폴더, 캐시 시간, 결과 개수,
최근 목록 개수, 포커스 아웃 숨김, 테마, 자동 시작, 복사 형식, `[[action]]` 사용자 액션을 설정한다.
최근 목록은 `%APPDATA%\fdpick\recent.json`, 마지막 모드는 `%APPDATA%\fdpick\state.json`.

## 구조

- `core/` (`fdpick-core`): UI 와 무관한 엔진. `fd`(실행, 스트리밍, 취소), `matcher`(nucleo), `pathnav`(경로 탐색),
  `recents`, `config`, `actions`(복사, 열기, 경로 정규화), `user_actions`, `engine`(질의 조율, 결과는 콜백으로 전달).
- `logic/` (`fdpick-logic`): 모드, 키, 선택, 상태줄을 다루는 순수 상태 머신(`Controller`). 창 시스템과 무관해 단위 테스트가 가능하다.
- `app/` (`fdpick`): Win32 셸. 창, 시스템 EDIT 입력(한글 IME), GDI 더블버퍼 목록, 트레이, 전역 단축키, 단일 인스턴스, 자동 시작.
- `spike/`: UI 후보(Win32, Slint) 프로토타입과 메모리 측정 스크립트(`measure.ps1`). 빌드에는 쓰이지 않는다.
- `reference/`: 초기 PowerShell+AutoHotkey 프로토타입(읽기 전용 참고).
