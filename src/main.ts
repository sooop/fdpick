import {
  applicableActions,
  doAction,
  hidePopup,
  listActions,
  onPopupShown,
  onResults,
  queryChanged,
  runUserAction,
  startupNotes,
  type ActionInfo,
  type ActionKind,
  type ResultItem,
  type Mode,
  type ResultsUpdated,
} from "./ipc";
import { VirtualList } from "./list";
import { ensureTrailingSep, folderOf, normalizeCopyPath, parentInput } from "./pathutil";

const input = document.getElementById("q") as HTMLInputElement;
const modeChip = document.getElementById("mode") as HTMLElement;
const helpEl = document.getElementById("help") as HTMLElement;
const statusEl = document.getElementById("status") as HTMLElement;
const listEl = document.getElementById("list") as HTMLElement;

const list = new VirtualList(listEl);

// IME(한글 등) 조합 중인지 추적한다.
// 조합 중에는 keydown 의 Enter/Esc 등이 조합 확정용이므로 명령으로 처리하지 않는다.
let composing = false;
const MODE_KEY = "fdpick.mode";

// 마지막에 사용한 모드를 읽는다. 기록이 없거나 접근 불가면 경로 모드.
function loadMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "fuzzy" ? "fuzzy" : "path";
  } catch {
    return "path";
  }
}

// recent 는 일회성이라 저장하지 않는다(fuzzy/path 만 기억).
function saveMode(m: Mode): void {
  if (m !== "fuzzy" && m !== "path") {
    return;
  }
  try {
    localStorage.setItem(MODE_KEY, m);
  } catch {
    // 저장소 접근 불가여도 동작은 계속한다.
  }
}

let mode: Mode = loadMode();

// 마지막으로 반영한 results_updated 의 seq
let lastSeq = -1;
let lastPayload: ResultsUpdated | null = null;
// 액션 실패 등 임시 상태 메시지(다음 입력/결과 때까지 우선 표시)
let statusOverride: string | null = null;
// 시작 시 알림(설정 경고 등). 첫 입력 전까지 상태줄에 남긴다
let startupNote: string | null = null;

const HELP: Record<Mode, string> = {
  fuzzy: "[fd옵션] -- 검색어 \u00a0 예) -e pdf -t f -- 보고서 · Tab 폴더 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
  path: "폴더 탐색 · Tab 또는 / 진입 · Shift+Tab 상위 · Enter 복사 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
  recent: "최근 사용 · 퍼지 검색 · Enter 복사 · Tab 폴더 · Ctrl+Enter 탐색기 · Ctrl+L 모드",
};
const MENU_HELP = "액션 메뉴 · 위/아래 선택 · Enter 실행 · Esc 닫기";

// 사용자 액션 상태: 전체 목록(단축키용), 단축키 맵, 메뉴 상태
let hasActions = false;
const hotkeys = new Map<string, ActionInfo>();
let menuOpen = false;
let menuActions: ActionInfo[] = [];
let menuTargets: string[] = [];
// 메뉴를 열기 직전의 커서 상태(닫을 때 복원)
let menuSavedCursor = { idx: 0, moved: false };

function updateHelp(): void {
  if (menuOpen) {
    helpEl.textContent = MENU_HELP;
    return;
  }
  helpEl.textContent = HELP[mode] + (hasActions ? " · Ctrl+K 액션" : "");
}

const PLACEHOLDER: Record<Mode, string> = {
  fuzzy: "[fd옵션] -- 검색어",
  path: "경로 입력 (예: d:\\work)",
  recent: "최근 사용 항목 검색",
};
const MODE_LABEL: Record<Mode, string> = { fuzzy: "검색", path: "경로", recent: "최근" };
// Ctrl+L 순환 순서
const NEXT_MODE: Record<Mode, Mode> = { fuzzy: "path", path: "recent", recent: "fuzzy" };

// ---------- 상태줄 ----------

const fmt = (n: number): string => n.toLocaleString("en-US");

function fmtElapsed(ms: number): string {
  return ms < 100 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
}

function renderStatus(): void {
  statusEl.classList.remove("err", "run");
  const parts: string[] = [];
  if (menuOpen) {
    parts.push("액션 메뉴");
  }
  if (list.picked.size > 0) {
    parts.push(`${fmt(list.picked.size)}개 선택`);
  }

  if (startupNote !== null) {
    statusEl.classList.add("err");
    parts.push(startupNote);
  }

  if (statusOverride !== null) {
    statusEl.classList.add("err");
    parts.push(statusOverride);
  } else if (lastPayload === null) {
    parts.push("준비됨");
  } else {
    const p = lastPayload;
    const t = fmtElapsed(p.elapsed_ms);
    if (p.state === "error") {
      statusEl.classList.add("err");
      parts.push(p.message ?? "오류");
    } else if (mode === "recent") {
      parts.push(`${fmt(p.matched)}개 일치`);
    } else if (p.state === "running") {
      statusEl.classList.add("run");
      parts.push(`검색 중… ${fmt(p.scanned)}개 스캔 · ${fmt(p.matched)}개 일치 · ${t}`);
    } else if (p.state === "done") {
      parts.push(`완료 · ${fmt(p.matched)}개 일치 / ${fmt(p.scanned)}개 스캔 · ${t}`);
    } else {
      parts.push("준비됨");
    }
    if (p.state !== "error" && p.message) {
      parts.push(p.message);
    }
    if (p.cache_note) {
      parts.push(p.cache_note);
    }
  }
  const text = parts.join(" · ");
  statusEl.textContent = text;
  statusEl.title = text;
}

list.onPickedChange = renderStatus;

// ---------- 결과 수신 (rAF 로 합침) ----------

let pendingPayload: ResultsUpdated | null = null;
let renderRaf = 0;

// 입력 끝의 이름 필터(마지막 구분자 뒤). 구분자가 없으면 입력 전체.
function typedFilter(): string {
  const v = input.value;
  return v.slice(Math.max(v.lastIndexOf("\\"), v.lastIndexOf("/")) + 1);
}

// 경로 모드에서 입력한 경로가 존재하지 않으면 입력 글자를 빨갛게 표시한다:
// 폴더를 열 수 없거나(상위 폴더 없음), 이름 필터와 일치하는 폴더가 하나도 없는 경우.
function updateInvalid(p: ResultsUpdated): void {
  const invalid =
    mode === "path" &&
    input.value.trim() !== "" &&
    (p.state === "error" || (typedFilter() !== "" && !p.items.some((it) => !it.recent)));
  input.classList.toggle("invalid", invalid);
}

function applyPayload(p: ResultsUpdated): void {
  lastPayload = p;
  statusOverride = null;
  updateInvalid(p);
  list.setItems(p.items);
  const emptyText =
    mode === "recent" && input.value.trim() === ""
      ? "최근 사용 기록이 없습니다"
      : "일치하는 항목이 없습니다";
  list.setEmptyHint(
    p.items.length === 0 && (p.state === "done" || p.state === "idle"),
    emptyText,
  );
  renderStatus();
}

void onResults((p) => {
  // 오래된 seq 와 현재 모드가 아닌 결과는 버린다.
  if (p.seq <= lastSeq || p.mode !== mode) {
    return;
  }
  lastSeq = p.seq;
  pendingPayload = p;
  if (!renderRaf) {
    renderRaf = requestAnimationFrame(() => {
      renderRaf = 0;
      if (pendingPayload) {
        const next = pendingPayload;
        pendingPayload = null;
        if (menuOpen) {
          // 메뉴 중에는 목록을 덮어쓰지 않고 닫을 때 반영한다
          lastPayload = next;
          return;
        }
        applyPayload(next);
      }
    });
  }
});

// ---------- query_changed 전송 (rAF 로 합침, 마지막 값은 반드시 전송) ----------

let queryRaf = 0;
let pendingForce = false;

function sendQuery(force = false): void {
  if (queryRaf) {
    cancelAnimationFrame(queryRaf);
    queryRaf = 0;
  }
  const f = force || pendingForce;
  pendingForce = false;
  queryChanged(input.value, mode, f).catch((err) => {
    statusOverride = String(err);
    renderStatus();
  });
}

function scheduleQuery(): void {
  if (queryRaf) {
    return;
  }
  queryRaf = requestAnimationFrame(() => {
    queryRaf = 0;
    sendQuery();
  });
}

// ---------- 모드/입력 조작 ----------

function applyModeUi(): void {
  modeChip.textContent = MODE_LABEL[mode];
  modeChip.classList.toggle("path", mode === "path");
  modeChip.classList.toggle("recent", mode === "recent");
  list.showRecentBadge = mode === "path";
  updateHelp();
  input.placeholder = PLACEHOLDER[mode];
}

// 입력을 바꾸고 커서를 끝으로 옮긴 뒤 즉시 질의한다.
// 경로 모드에서 폴더를 가리키는 입력(끝 구분자 또는 드라이브만)이면 초기 선택을 두지 않는다.
// 그래야 Enter 가 목록의 첫 항목이 아니라 방금 들어온 그 폴더를 복사한다.
function applyCursorPolicy(): void {
  const t = input.value.trim();
  const pointsAtFolder = mode === "path" && t !== "" && /(?:[\\/]|^[A-Za-z]:?)$/.test(t);
  if (pointsAtFolder) {
    list.hideCursor();
  } else {
    list.showCursor();
  }
}

function setInput(text: string): void {
  input.value = text;
  input.setSelectionRange(text.length, text.length);
  statusOverride = null;
  list.resetUserMoved();
  applyCursorPolicy();
  sendQuery();
}

function switchMode(next: Mode): void {
  closeMenu(true);
  input.classList.remove("invalid");
  mode = next;
  saveMode(mode);
  input.value = "";
  statusOverride = null;
  list.picked.clear();
  list.resetUserMoved();
  applyModeUi();
  renderStatus();
  sendQuery();
}

function resetToInitial(): void {
  closeMenu(false);
  input.classList.remove("invalid");
  // 조합 도중 팝업이 숨겨지면 compositionend 가 오지 않아 플래그가 남을 수 있다
  composing = false;
  mode = loadMode();
  input.value = "";
  statusOverride = null;
  lastPayload = null;
  pendingPayload = null;
  list.picked.clear();
  list.clear();
  applyModeUi();
  renderStatus();
  sendQuery();
  input.focus();
}

// 커서 항목으로 입력을 완성하고 한 단계 진입(path 모드). 항목이 없으면 false.
function enterCursorDir(): boolean {
  const it = list.current();
  if (!it) {
    return false;
  }
  setInput(ensureTrailingSep(it.path));
  return true;
}

// 경로 모드에서 이름을 입력하던 중(`C:/apps/b`)에 Enter 를 누르면 복사하지 않고 Tab 처럼
// 강조 항목(`bin`)으로 완성해 진입한다. 한 번 더 Enter 를 눌러야 그 폴더를 복사한다.
// 직접 고른 항목, 다중 선택, 입력과 이름이 이미 같은 경우, 최근 항목이 대상이면 바로 복사한다.
function completeOnEnter(): boolean {
  if (mode !== "path" || list.picked.size > 0 || list.moved) {
    return false;
  }
  const it = list.current();
  if (!it || it.recent) {
    return false;
  }
  const typed = typedFilter();
  if (typed === "") {
    return false;
  }
  const name = it.path.slice(it.name_start);
  if (name.toLowerCase() === typed.toLowerCase()) {
    return false;
  }
  return enterCursorDir();
}

// Shift+Tab: 경로 모드에서 한 단계 상위 폴더로 올라간다. 다른 모드에서는 무시한다.
function goBack(): void {
  if (mode === "path" && input.value !== "") {
    setInput(parentInput(input.value));
  }
}

// fuzzy/recent 모드 Tab, 경로 모드의 최근 항목 진입: 커서 항목의 폴더로 경로 모드 시작
function fuzzyToPath(): void {
  const it = list.current();
  if (!it) {
    return;
  }
  const dir = ensureTrailingSep(folderOf(it.path, it.is_dir));
  mode = "path";
  saveMode(mode);
  list.picked.clear();
  applyModeUi();
  setInput(dir);
  renderStatus();
}

// ---------- 액션 ----------

function actionTargets(): string[] {
  if (list.picked.size > 0) {
    return [...list.picked];
  }
  const it = list.current();
  if (it) {
    return [it.path];
  }
  if (mode === "path") {
    const p = normalizeCopyPath(input.value);
    return p === "" ? [] : [p];
  }
  return [];
}

function runAction(kind: ActionKind): void {
  const paths = actionTargets();
  if (paths.length === 0) {
    return;
  }
  doAction(kind, paths).then(
    () => {
      void hidePopup();
    },
    (err) => {
      statusOverride = String(err);
      renderStatus();
    },
  );
}

// 사용자 액션 실행. 성공 시 close_popup 이면 팝업을 닫고, 실패 시 상태줄에 표시한다.
function runUser(act: ActionInfo, paths: string[]): void {
  runUserAction(act.id, paths).then(
    () => {
      if (act.close_popup) {
        void hidePopup();
      }
    },
    (err) => {
      statusOverride = String(err);
      renderStatus();
    },
  );
}

// 눌린 키 조합을 백엔드와 같은 정규화 형식("Ctrl+Alt+Shift+키")으로 만든다.
function comboOf(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) {
    parts.push("Ctrl");
  }
  if (e.altKey) {
    parts.push("Alt");
  }
  if (e.shiftKey) {
    parts.push("Shift");
  }
  // 공백 키는 e.key 가 " " 이므로 이름으로 바꾼다(설정 파일의 Space 와 일치)
  const k = e.key === " " ? "Space" : e.key.length === 1 ? e.key.toUpperCase() : e.key;
  parts.push(k);
  return parts.join("+");
}

// 액션 메뉴 열기: 현재 대상에 적용 가능한 액션을 목록 자리에 보여준다.
function openMenu(): void {
  const paths = actionTargets();
  if (paths.length === 0) {
    return;
  }
  applicableActions(paths).then(
    (acts) => {
      if (menuOpen) {
        return;
      }
      if (acts.length === 0) {
        statusOverride = "적용할 수 있는 액션이 없습니다";
        renderStatus();
        return;
      }
      menuSavedCursor = { idx: list.cursor, moved: list.moved };
      menuActions = acts;
      menuTargets = paths;
      menuOpen = true;
      const items: ResultItem[] = acts.map((a) => ({
        path: a.name,
        name_start: 0,
        indices: [],
        is_dir: false,
      }));
      list.hints = acts.map((a) => a.key);
      list.showCursor();
      list.setItems(items);
      list.restoreCursor(0, false);
      list.setEmptyHint(false);
      updateHelp();
      renderStatus();
    },
    (err) => {
      statusOverride = String(err);
      renderStatus();
    },
  );
}

// 메뉴 닫기. restore 가 true 면 이전 목록/커서를 복원한다(팝업 재표시·모드 전환 때는 호출 쪽이 다시 채운다).
function closeMenu(restore: boolean): void {
  if (!menuOpen) {
    return;
  }
  menuOpen = false;
  menuActions = [];
  menuTargets = [];
  list.hints = null;
  if (restore) {
    if (lastPayload) {
      applyPayload(lastPayload);
    } else {
      list.clear();
    }
    list.restoreCursor(menuSavedCursor.idx, menuSavedCursor.moved);
  }
  updateHelp();
  renderStatus();
}

// 메뉴가 열려 있는 동안의 키 처리
function handleMenuKey(e: KeyboardEvent): void {
  const key = e.key;
  const ctrl = e.ctrlKey && !e.altKey && !e.metaKey;
  const lower = key.length === 1 ? key.toLowerCase() : key;
  if (key === "Escape") {
    e.preventDefault();
    closeMenu(true);
  } else if (key === "Enter") {
    e.preventDefault();
    const act = menuActions[list.cursor];
    if (act && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      runUser(act, menuTargets);
    }
  } else if (key === "ArrowUp" || (ctrl && lower === "p")) {
    e.preventDefault();
    list.move(-1);
  } else if (key === "ArrowDown" || (ctrl && lower === "n")) {
    e.preventDefault();
    list.move(1);
  } else if (key === "PageUp") {
    e.preventDefault();
    list.move(-list.pageSize());
  } else if (key === "PageDown") {
    e.preventDefault();
    list.move(list.pageSize());
  } else if (
    key === "Tab" ||
    key === "/" ||
    key === "\\" ||
    (ctrl && (e.code === "Space" || key === " " || ["d", "o", "l", "r", "k"].includes(lower))) ||
    (e.altKey && key === "Enter")
  ) {
    // 메뉴 중 내장 명령은 무시(Tab 의 포커스 이동도 막는다)
    e.preventDefault();
  }
}

// 시작 시 전체 액션을 가져와 단축키 맵과 도움말을 준비한다.
listActions()
  .then((acts) => {
    hasActions = acts.length > 0;
    for (const a of acts) {
      if (a.key !== "") {
        hotkeys.set(a.key, a);
      }
    }
    updateHelp();
  })
  .catch(() => {});

// ---------- 입력/키 ----------

input.addEventListener("compositionstart", () => {
  composing = true;
});

input.addEventListener("compositionend", () => {
  composing = false;
});

// 조합 중에도 입력 이벤트는 처리해 질의한다.
input.addEventListener("input", () => {
  if (menuOpen) {
    return;
  }
  statusOverride = null;
  startupNote = null;
  list.resetUserMoved();
  applyCursorPolicy();
  scheduleQuery();
});

// 명령 키 처리. 조합 중(e.isComposing 또는 composing)이면 무시한다.
document.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.isComposing || composing) {
    return;
  }
  if (menuOpen) {
    handleMenuKey(e);
    return;
  }
  const key = e.key;
  const ctrl = e.ctrlKey && !e.altKey && !e.metaKey;
  const lower = key.length === 1 ? key.toLowerCase() : key;

  if (ctrl && lower === "k") {
    e.preventDefault();
    openMenu();
    return;
  }

  if (key === "Tab") {
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey) {
      return;
    }
    if (e.shiftKey) {
      goBack();
      return;
    }
    if (mode === "path" && !list.current()?.recent) {
      enterCursorDir();
    } else {
      fuzzyToPath();
    }
    return;
  }

  if (key === "Escape") {
    e.preventDefault();
    void hidePopup();
    return;
  }

  if (key === "Enter") {
    e.preventDefault();
    if (e.altKey && !e.ctrlKey) {
      runAction("open");
    } else if (e.ctrlKey && !e.altKey && !e.shiftKey) {
      // Ctrl+Enter: Ctrl+O 와 같은 탐색기 열기
      runAction("reveal");
    } else if (!e.ctrlKey && !e.shiftKey) {
      if (!completeOnEnter()) {
        runAction("copy");
      }
    }
    return;
  }

  if (key === "ArrowUp" || (ctrl && lower === "p")) {
    e.preventDefault();
    list.move(-1);
    return;
  }
  if (key === "ArrowDown" || (ctrl && lower === "n")) {
    e.preventDefault();
    list.move(1);
    return;
  }
  if (key === "PageUp") {
    e.preventDefault();
    list.move(-list.pageSize());
    return;
  }
  if (key === "PageDown") {
    e.preventDefault();
    list.move(list.pageSize());
    return;
  }

  if (ctrl && (e.code === "Space" || key === " ")) {
    e.preventDefault();
    list.togglePickAndAdvance();
    return;
  }
  if (ctrl && lower === "l") {
    e.preventDefault();
    switchMode(NEXT_MODE[mode]);
    return;
  }
  if (ctrl && lower === "d") {
    e.preventDefault();
    runAction("copy_dir");
    return;
  }
  if (ctrl && lower === "o") {
    e.preventDefault();
    runAction("reveal");
    return;
  }
  if (ctrl && lower === "r") {
    e.preventDefault();
    sendQuery(true);
    return;
  }

  // 사용자 액션 단축키
  if (!e.metaKey && hotkeys.size > 0) {
    const act = hotkeys.get(comboOf(e));
    if (act) {
      e.preventDefault();
      const paths = actionTargets();
      if (paths.length > 0) {
        runUser(act, paths);
      }
      return;
    }
  }

  // 경로 모드의 구분자: 입력 끝에 커서가 있을 때만 완성/진입으로 처리한다.
  if (
    mode === "path" &&
    (key === "/" || key === "\\") &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey
  ) {
    const atEnd =
      input.selectionStart === input.value.length && input.selectionEnd === input.value.length;
    if (!atEnd) {
      return;
    }
    e.preventDefault();
    // 구분자 키는 섣불리 완성하지 않는다. 입력이 비었으면 아무것도 하지 않고,
    // 최근 항목은 사용자가 직접 골랐을 때만 진입하며, 그 외에는 구분자만 붙인다.
    if (input.value.trim() === "") {
      return;
    }
    const insertSep = (): void => {
      if (!/[\\/]$/.test(input.value)) {
        setInput(input.value + "\\");
      }
    };
    if (list.current()?.recent) {
      if (list.moved) {
        fuzzyToPath();
      } else {
        insertSep();
      }
    } else if (!enterCursorDir()) {
      insertSep();
    }
  }
});

// 팝업이 표시될 때마다 Rust 쪽에서 popup-shown 이벤트를 보낸다.
window.addEventListener("blur", () => {
  composing = false;
});

void onPopupShown(resetToInitial);

// 시작 알림은 백엔드 준비 후 한 번 가져온다
startupNotes()
  .then((notes) => {
    if (notes.length > 0) {
      startupNote = notes.join(" · ");
      renderStatus();
    }
  })
  .catch(() => {});

// 초기 상태
applyModeUi();
renderStatus();
sendQuery();
