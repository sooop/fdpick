import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// 백엔드와 합의한 IPC 계약 타입.

export type Mode = "fuzzy" | "path" | "recent";

export interface ResultItem {
  // 정규화된 경로(끝 구분자 제거)
  path: string;
  // path 안에서 파일명이 시작하는 char(코드포인트) 인덱스
  name_start: number;
  // 매치된 글자의 코드포인트 인덱스(오름차순)
  indices: number[];
  is_dir: boolean;
  // 최근 사용 항목 여부(없을 수 있음)
  recent?: boolean;
  // 경로가 더 이상 존재하지 않음(없을 수 있음)
  missing?: boolean;
}

export interface ResultsUpdated {
  // 단조 증가. 이전 seq 이하의 이벤트는 버린다.
  seq: number;
  mode: Mode;
  items: ResultItem[];
  matched: number;
  scanned: number;
  state: "idle" | "running" | "done" | "error";
  elapsed_ms: number;
  message: string | null;
  cache_note: string | null;
}

export type ActionKind = "copy" | "copy_dir" | "reveal" | "open";

export function queryChanged(text: string, mode: Mode, force?: boolean): Promise<void> {
  const args: { text: string; mode: Mode; force?: boolean } = { text, mode };
  if (force) {
    args.force = true;
  }
  return invoke<void>("query_changed", args);
}

// 사용자 정의 액션 정보. key 는 "Ctrl+Shift+V" 형식(없으면 빈 문자열)
export interface ActionInfo {
  id: number;
  name: string;
  key: string;
  batch: boolean;
  close_popup: boolean;
}

export function listActions(): Promise<ActionInfo[]> {
  return invoke<ActionInfo[]>("list_actions");
}

export function applicableActions(paths: string[]): Promise<ActionInfo[]> {
  return invoke<ActionInfo[]>("applicable_actions", { paths });
}

export function runUserAction(id: number, paths: string[]): Promise<void> {
  return invoke<void>("run_user_action", { id, paths });
}

export function doAction(kind: ActionKind, paths: string[]): Promise<void> {
  return invoke<void>("do_action", { kind, paths });
}

/** 시작 시 알림(설정 경고, 단축키 등록 실패 등) */
export function startupNotes(): Promise<string[]> {
  return invoke<string[]>("startup_notes");
}

export function hidePopup(): Promise<void> {
  return invoke<void>("hide_popup");
}

export function onResults(handler: (payload: ResultsUpdated) => void): Promise<() => void> {
  return listen<ResultsUpdated>("results_updated", (e) => handler(e.payload));
}

export function onPopupShown(handler: () => void): Promise<() => void> {
  return listen("popup-shown", () => handler());
}
