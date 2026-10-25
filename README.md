# fdpick

Windows 용 Spotlight 스타일 경로 검색기. 인덱스 없이 `fd` 로 파일시스템을 훑고 내장 fuzzy 매처(nucleo)로 거른 뒤,
선택한 경로를 클립보드에 복사하거나 탐색기로 연다. Tauri 2 + Rust 백엔드 + 바닐라 TypeScript(Vite).

## 개발 환경

- Rust(`stable-x86_64-pc-windows-gnu`) + MSYS2 mingw64(`dlltool`, `windres`, `gcc` 가 PATH 에 있어야 함). MSVC 불필요.
- Node.js, WebView2(Windows 11 기본 포함), `fd`(PATH 또는 설정의 `fd_path`).
- 글꼴: 입력창은 설치된 `Jetendard` 를 쓴다(없으면 JetBrains Mono, Pretendard 순으로 대체). 웹폰트 없음.

## 실행과 빌드

```
npm install
npx tauri dev            # 개발 실행(개발 서버를 직접 관리하는 경우 그 터미널에서)
npx tauri build          # 릴리스 빌드(NSIS 설치 파일 포함)
cd src-tauri && cargo test
```

단축키가 다른 앱과 겹치면 환경변수 `FDPICK_HOTKEY="Ctrl+Alt+F"` 로 임시 지정하거나 설정 파일의 `hotkey` 를 바꾼다.
등록에 실패해도 앱은 트레이로 계속 실행된다.

## 모드와 키

| 모드 | 용도 |
|---|---|
| 검색 | `[fd옵션] -- 검색어` 로 fd 결과를 fuzzy 검색 |
| 경로 | 폴더를 한 단계씩 내려가며 경로를 만든다(입력이 비면 드라이브 목록, 입력 중이면 최근 항목 5개를 위에 표시) |
| 최근 | 복사/열기한 경로의 최근 사용 목록(fuzzy 검색) |

Ctrl+L 모드 순환(검색 -> 경로 -> 최근), Enter 복사, Ctrl+D 폴더 경로 복사, Ctrl+O 또는 Ctrl+Enter 탐색기 열기,
Alt+Enter 기본 프로그램 열기, Ctrl+Space 다중 선택, Tab 진입/폴더 이동, Shift+Tab 뒤로, Ctrl+R 새로고침, Esc 닫기.
상세 사양은 `docs/path-mode.md`.

## 설정

`%APPDATA%\fdpick\config.toml`(없으면 첫 실행에 생성). 최근 목록은 `%APPDATA%\fdpick\recent.json`.

## 구조

`src-tauri/src`: `main.rs`(창, 트레이, 단축키, 자동 시작), `engine.rs`(질의 조율), `query.rs`(질의 분리, 인자 토크나이저),
`fd.rs`(fd 실행, 스트리밍, 취소), `matcher.rs`(nucleo 래퍼), `pathnav.rs`(경로 탐색), `recents.rs`(최근 사용),
`actions.rs`(복사, 열기, 경로 정규화), `config.rs`(설정).
