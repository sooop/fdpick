import type { ResultItem } from "./ipc";
import { splitHighlight, type Seg } from "./pathutil";

// 행 높이(px). style.css 의 .row height 와 반드시 일치해야 한다.
export const ROW_HEIGHT = 29;
// 뷰포트 위아래로 추가 렌더하는 행 수
const BUFFER = 4;

function appendSegs(parent: HTMLElement, segs: Seg[]): void {
  for (const s of segs) {
    if (s.hit) {
      const m = document.createElement("mark");
      m.textContent = s.text;
      parent.appendChild(m);
    } else {
      parent.appendChild(document.createTextNode(s.text));
    }
  }
}

// 가상 스크롤 목록. 뷰포트 근처 행만 DOM 으로 만든다(변경 시 단순 재구성).
export class VirtualList {
  private readonly host: HTMLElement;
  private readonly spacer: HTMLDivElement;
  private readonly hint: HTMLDivElement;
  private items: ResultItem[] = [];
  private cursorIdx = 0;
  // 사용자가 방향키로 직접 움직였는지(같은 path 유지 판단용)
  private userMoved = false;
  // 초기 선택 없음(cursorIdx = -1). 방향키/클릭 전까지 Enter 는 목록 항목이 아닌 입력 경로를 대상으로 한다
  private noCursor = false;
  private raf = 0;
  readonly picked = new Set<string>();
  // 선택 집합/커서가 바뀌었을 때 호출(상태줄 갱신용)
  onPickedChange: (() => void) | null = null;

  constructor(host: HTMLElement) {
    this.host = host;
    this.spacer = document.createElement("div");
    this.spacer.className = "spacer";
    this.hint = document.createElement("div");
    this.hint.className = "empty";
    this.hint.textContent = "일치하는 항목이 없습니다";
    this.hint.hidden = true;
    host.append(this.spacer, this.hint);

    host.addEventListener("scroll", () => this.schedule());
    window.addEventListener("resize", () => this.schedule());
    // 입력창 포커스를 빼앗기지 않도록 한다.
    host.addEventListener("mousedown", (e) => e.preventDefault());
  }

  get length(): number {
    return this.items.length;
  }

  get cursor(): number {
    return this.cursorIdx;
  }

  // 마지막 입력 이후 사용자가 방향키/클릭으로 직접 항목을 골랐는지
  get moved(): boolean {
    return this.userMoved;
  }

  current(): ResultItem | null {
    return this.items[this.cursorIdx] ?? null;
  }

  // 선택 없음 상태로 만든다(폴더에 막 진입했을 때)
  hideCursor(): void {
    this.noCursor = true;
    this.cursorIdx = -1;
    this.host.scrollTop = 0;
    this.schedule();
  }

  // 선택 없음 상태를 풀고 다음 결과부터 첫 항목을 선택한다
  showCursor(): void {
    this.noCursor = false;
  }

  // 메뉴를 닫은 뒤 이전 커서 위치를 복원한다(idx < 0 이면 선택 없음)
  restoreCursor(idx: number, moved: boolean): void {
    this.userMoved = moved;
    if (idx < 0 || this.items.length === 0) {
      this.noCursor = idx < 0;
      this.cursorIdx = idx < 0 ? -1 : 0;
    } else {
      this.noCursor = false;
      this.cursorIdx = Math.min(idx, this.items.length - 1);
    }
    this.ensureVisible();
    this.schedule();
  }

  // 새 입력/모드 전환 후에는 이전 커서 위치를 유지하지 않는다.
  resetUserMoved(): void {
    this.userMoved = false;
  }

  // 최근 배지/구분선 표시 여부(경로 모드 true, 최근 모드 false)
  showRecentBadge = true;

  // 행별 보조 문구(액션 메뉴의 단축키 표시용). setItems 전에 지정하고, 평소엔 null
  hints: string[] | null = null;

  setEmptyHint(show: boolean, text = "일치하는 항목이 없습니다"): void {
    this.hint.textContent = text;
    this.hint.hidden = !show;
  }

  // 결과 교체. 사용자가 움직였으면 같은 path 를 찾아 커서를 유지, 아니면 0.
  setItems(items: ResultItem[]): void {
    const prevPath = this.current()?.path;
    this.items = items;
    const first = this.defaultIndex(items);
    let next = this.noCursor ? -1 : first;
    if (!this.noCursor && this.userMoved && prevPath !== undefined) {
      const found = items.findIndex((it) => it.path === prevPath);
      next = found >= 0 ? found : first;
    }
    this.cursorIdx = next;
    this.spacer.style.height = `${items.length * ROW_HEIGHT}px`;
    if (next <= 0) {
      this.host.scrollTop = 0;
    }
    this.ensureVisible();
    this.schedule();
  }

  // 직접 고르기 전의 기본 선택. 경로 모드에서는 맨 위의 최근 항목을 건너뛰고 첫 폴더 항목을 선택한다
  // (최근 항목은 방향키로 직접 골라야 한다). 폴더 항목이 없으면 첫 항목.
  private defaultIndex(items: ResultItem[]): number {
    if (!this.showRecentBadge) {
      return 0;
    }
    const i = items.findIndex((it) => !it.recent);
    return i >= 0 ? i : 0;
  }

  clear(): void {
    this.items = [];
    this.cursorIdx = 0;
    this.userMoved = false;
    this.noCursor = false;
    this.spacer.style.height = "0px";
    this.host.scrollTop = 0;
    this.setEmptyHint(false);
    this.schedule();
  }

  clearPicked(): void {
    this.picked.clear();
    this.schedule();
    this.onPickedChange?.();
  }

  // 커서 이동(끝에서 순환하지 않음)
  move(delta: number): void {
    if (this.items.length === 0) {
      return;
    }
    this.userMoved = true;
    if (this.noCursor) {
      // 선택 없음에서 처음 움직이면 위쪽 이동은 마지막, 아래쪽 이동은 첫 항목으로
      this.noCursor = false;
      this.cursorIdx = delta > 0 ? 0 : this.items.length - 1;
    } else {
      this.cursorIdx = Math.max(0, Math.min(this.items.length - 1, this.cursorIdx + delta));
    }
    this.ensureVisible();
    this.schedule();
  }

  // 한 화면에 들어가는 행 수
  pageSize(): number {
    return Math.max(1, Math.floor(this.viewHeight() / ROW_HEIGHT));
  }

  // 커서 항목의 선택 토글 후 한 칸 아래로
  togglePickAndAdvance(): void {
    const it = this.current();
    if (!it) {
      return;
    }
    if (this.picked.has(it.path)) {
      this.picked.delete(it.path);
    } else {
      this.picked.add(it.path);
    }
    this.onPickedChange?.();
    this.move(1);
    this.schedule();
  }

  private viewHeight(): number {
    return this.host.clientHeight || 400;
  }

  // 커서 행이 보이도록 scrollTop 을 조정한다.
  private ensureVisible(): void {
    if (this.cursorIdx < 0) {
      return;
    }
    const top = this.cursorIdx * ROW_HEIGHT;
    const h = this.viewHeight();
    const st = this.host.scrollTop;
    if (top < st) {
      this.host.scrollTop = top;
    } else if (top + ROW_HEIGHT > st + h) {
      this.host.scrollTop = top + ROW_HEIGHT - h;
    }
  }

  private schedule(): void {
    if (this.raf) {
      return;
    }
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.render();
    });
  }

  private render(): void {
    const n = this.items.length;
    const st = this.host.scrollTop;
    const h = this.viewHeight();
    const start = Math.max(0, Math.floor(st / ROW_HEIGHT) - BUFFER);
    const end = Math.min(n, Math.ceil((st + h) / ROW_HEIGHT) + BUFFER);
    const frag = document.createDocumentFragment();
    for (let i = start; i < end; i++) {
      frag.appendChild(this.buildRow(i, this.items[i]));
    }
    this.spacer.replaceChildren(frag);
  }

  private buildRow(i: number, it: ResultItem): HTMLDivElement {
    const row = document.createElement("div");
    row.className = "row";
    if (i === this.cursorIdx) {
      row.classList.add("sel");
    }
    if (this.picked.has(it.path)) {
      row.classList.add("picked");
    }
    if (it.missing) {
      row.classList.add("missing");
    }
    // 경로 모드의 최근 구역 마지막 항목 아래에만 구분선(최근 모드에서는 쓰지 않음)
    if (this.showRecentBadge && it.recent && !this.items[i + 1]?.recent) {
      row.classList.add("recent-last");
    }
    row.style.top = `${i * ROW_HEIGHT}px`;
    row.addEventListener("click", () => {
      this.userMoved = true;
      this.noCursor = false;
      this.cursorIdx = i;
      this.schedule();
    });

    const ico = document.createElement("span");
    ico.className = "ico";
    ico.textContent = it.is_dir ? "▸" : "";

    const { dir, name } = splitHighlight(it.path, it.name_start, it.indices);
    const dirEl = document.createElement("span");
    dirEl.className = "dir";
    // rtl 바깥 + ltr 안쪽으로 왼쪽부터 잘리게 하되 글자 순서는 유지한다.
    const dirIn = document.createElement("span");
    dirIn.className = "dir-in";
    appendSegs(dirIn, dir);
    dirEl.appendChild(dirIn);

    const nameEl = document.createElement("span");
    nameEl.className = "name";
    appendSegs(nameEl, name);

    if (this.showRecentBadge && it.recent) {
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = "최근";
      row.append(ico, badge, dirEl, nameEl);
      return row;
    }
    row.append(ico, dirEl, nameEl);
    const hintText = this.hints?.[i];
    if (hintText) {
      const h = document.createElement("span");
      h.className = "hint";
      h.textContent = hintText;
      row.appendChild(h);
    }
    return row;
  }
}
