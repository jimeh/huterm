/**
 * Reads and checks the session switcher beside the sidebar button, which
 * the palette smoke's title-bar and client-frame steps publish: its corner
 * host, its painted rect, and where the session list opened against it.
 * Pure, so the checks have unit tests.
 */
import { field, parseRect } from "./check-overlays";

export type Rect = { x: number; y: number; w: number; h: number };

/** The switcher's drawn height, and its width with only its icon. */
export const CONTROL_SIZE = 26;
/** The widest the switcher draws before its name truncates: a 140-point
 * label and the trigger's icon, chevron, and padding. */
export const CONTROL_MAX = 192;
/** The gap between the sidebar button and the switcher beside it. */
export const CORNER_GAP = 4;

export type SessionSwitcher = {
  host: string;
  rect: Rect;
  /** Only the layers icon shows. */
  icon: boolean;
  open: boolean;
  /** Where the session list painted, while open. */
  list?: Rect;
};

/** The `w0.session_*` fields of a state dump; `rect` is required. */
export function sessionSwitcher(state: string): SessionSwitcher {
  const rect = parseRect(field(state, "session_rect"));
  const list = field(state, "session_list_rect");
  return {
    host: field(state, "session_host"),
    rect,
    icon: Math.abs(rect.w - CONTROL_SIZE) <= 0.5,
    open: field(state, "session_open") === "true",
    list: list === "none" ? undefined : parseRect(list),
  };
}

const right = (rect: Rect) => rect.x + rect.w;
const bottom = (rect: Rect) => rect.y + rect.h;

/**
 * Problems with the switcher's placement: the expected host, a 26-point
 * height (and width with only its icon), no wider than its widest look,
 * inside `row`, and, when the sidebar button shows in the same row, just
 * after it on the left or just before it on the right, at the same height.
 */
export function placementProblems(
  switcher: SessionSwitcher,
  host: string,
  row: Rect,
  button?: Rect,
): string[] {
  const { rect } = switcher;
  const problems: string[] = [];
  if (switcher.host !== host) problems.push(`host ${switcher.host}, expected ${host}`);
  if (Math.abs(rect.h - CONTROL_SIZE) > 0.5) problems.push(`height ${rect.h}`);
  if (rect.w > CONTROL_MAX + 0.5) problems.push(`width ${rect.w} beyond ${CONTROL_MAX}`);
  if (rect.x < row.x - 0.5 || right(rect) > right(row) + 0.5 || rect.y < row.y - 0.5 || bottom(rect) > bottom(row) + 0.5) {
    problems.push(`rect ${JSON.stringify(rect)} outside ${JSON.stringify(row)}`);
  }
  if (button) {
    const gap = rect.x >= right(button) - 0.5 ? rect.x - right(button) : button.x - right(rect);
    if (gap < -0.5 || gap > CORNER_GAP + 4) problems.push(`leaves ${gap} points beside the sidebar button ${JSON.stringify(button)}`);
    if (Math.abs(rect.y - button.y) > 0.5) problems.push(`top ${rect.y}, sidebar button top ${button.y}`);
  }
  return problems;
}

/**
 * Problems with where the session list opened: below the switcher, or
 * above it from a bottom bar, and inside the window either way.
 */
export function listProblems(list: Rect, switcher: Rect, upward: boolean, window: Rect): string[] {
  const problems: string[] = [];
  if (upward ? bottom(list) > switcher.y + 0.5 : list.y < bottom(switcher) - 0.5) {
    problems.push(`list ${JSON.stringify(list)} is not ${upward ? "above" : "below"} the switcher ${JSON.stringify(switcher)}`);
  }
  if (list.x < window.x - 0.5 || right(list) > right(window) + 0.5) {
    problems.push(`list ${JSON.stringify(list)} leaves the window ${JSON.stringify(window)}`);
  }
  return problems;
}

/** Throws `label` and the problems when there are any. */
export function assertNone(label: string, problems: string[]): void {
  if (problems.length > 0) throw new Error(`${label}: ${problems.join("; ")}`);
}
