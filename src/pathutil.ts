// DOM 에 의존하지 않는 순수 경로/인덱스 처리 함수 모음.

export interface Seg {
  text: string;
  hit: boolean;
}

// "D:" 또는 "D:\" 형태의 드라이브 루트인지 확인한다.
export function isDriveRoot(p: string): boolean {
  return /^[A-Za-z]:\\?$/.test(p);
}

// 끝에 구분자가 없으면 `\` 를 붙인다(드라이브 루트 "D:\" 는 중복하지 않는다).
export function ensureTrailingSep(p: string): string {
  return p.endsWith("\\") ? p : p + "\\";
}

// 복사 대상 경로 정규화: `/` -> `\`, 끝 구분자 제거, 드라이브 루트는 `C:\` 유지.
// 빈 입력이면 빈 문자열을 돌려준다.
export function normalizeCopyPath(text: string): string {
  let t = text.trim().replace(/\//g, "\\");
  if (t === "") {
    return "";
  }
  // 드라이브 문자만(`d`, `d:`, `d:\\`)이면 루트로 취급한다.
  if (/^[A-Za-z]:?\\*$/.test(t)) {
    return t[0].toUpperCase() + ":\\";
  }
  // UNC 선두 `\\` 는 보존하고 끝 구분자만 제거한다.
  while (t.length > 1 && t.endsWith("\\")) {
    t = t.slice(0, -1);
  }
  return t;
}

// Shift+Tab 부모 폴더 계산: `/` -> `\` 정규화, 끝 구분자 제거, 마지막 조각 제거 후 `\` 부착.
// 드라이브 루트(`D:\`, `D:`, `D`)이거나 조각이 하나뿐이면 빈 문자열(드라이브 목록).
export function parentInput(text: string): string {
  let t = text.replace(/\//g, "\\");
  while (t.endsWith("\\")) {
    t = t.slice(0, -1);
  }
  if (t === "" || /^[A-Za-z]:?$/.test(t)) {
    return "";
  }
  const idx = t.lastIndexOf("\\");
  if (idx < 0) {
    return "";
  }
  return t.slice(0, idx + 1);
}

// 항목이 속한 폴더 경로(끝 구분자 없음, 단 드라이브 루트는 `D:\`).
// 폴더면 자기 자신, 파일이면 마지막 `\` 앞까지.
export function folderOf(path: string, isDir: boolean): string {
  if (isDir) {
    return path;
  }
  const idx = path.lastIndexOf("\\");
  if (idx < 0) {
    return path;
  }
  // `D:\file.txt` 처럼 드라이브 루트 바로 아래이면 `D:\` 를 유지한다.
  if (idx === 2 && path[1] === ":") {
    return path.slice(0, idx + 1);
  }
  return path.slice(0, idx);
}

// 같은 값의 연속 구간을 하나의 Seg 로 묶는다.
function runs(chars: string[], from: number, to: number, hitSet: Set<number>): Seg[] {
  const out: Seg[] = [];
  let buf = "";
  let cur = false;
  for (let i = from; i < to; i++) {
    const h = hitSet.has(i);
    if (buf !== "" && h !== cur) {
      out.push({ text: buf, hit: cur });
      buf = "";
    }
    cur = h;
    buf += chars[i];
  }
  if (buf !== "") {
    out.push({ text: buf, hit: cur });
  }
  return out;
}

// 코드포인트 단위로 경로를 상위 경로(dir)/파일명(name) 구간으로 나누고
// 매치 글자 인덱스를 각 구간에 분배한다.
export function splitHighlight(
  path: string,
  nameStart: number,
  indices: number[],
): { dir: Seg[]; name: Seg[] } {
  const chars = Array.from(path);
  const ns = Math.max(0, Math.min(nameStart, chars.length));
  const hitSet = new Set(indices);
  return {
    dir: runs(chars, 0, ns, hitSet),
    name: runs(chars, ns, chars.length, hitSet),
  };
}
