/**
 * Reads and checks the session control the palette smoke's title-bar and
 * client-frame steps publish: its host, its painted rect, and where the
 * session menu opened against it. Pure, so the checks have unit tests.
 */
import { field, parseRect } from "./check-overlays";

export type Rect = { x: number; y: number; w: number; h: number };

/** The control's drawn size: a 26-point control, like the menu button. */
export const CONTROL_SIZE = 26;
/** The widest the control draws before its name truncates. */
export const CONTROL_MAX = 168;

export type SessionControl = {
  host: string;
  rect: Rect;
  collapsed: boolean;
  truncated: boolean;
  open: boolean;
};

/** The `w0.session_*` fields of a state dump; `rect` is required. */
export function sessionControl(state: string): SessionControl {
  return {
    host: field(state, "session_host"),
    rect: parseRect(field(state, "session_rect")),
    collapsed: field(state, "session_collapsed") === "true",
    truncated: field(state, "session_truncated") === "true",
    open: field(state, "session_open") === "true",
  };
}

const right = (rect: Rect) => rect.x + rect.w;
const bottom = (rect: Rect) => rect.y + rect.h;

/**
 * Problems with the control's placement: the expected host, a 26-point
 * height (and width when collapsed), no wider than its widest look, inside
 * `row`, and, when the menu button shows, directly before it at the same
 * height, so the button keeps the rightmost slot.
 */
export function placementProblems(
  control: SessionControl,
  host: string,
  row: Rect,
  menu?: Rect,
): string[] {
  const { rect } = control;
  const problems: string[] = [];
  if (control.host !== host) problems.push(`host ${control.host}, expected ${host}`);
  if (Math.abs(rect.h - CONTROL_SIZE) > 0.5) problems.push(`height ${rect.h}`);
  if (control.collapsed && Math.abs(rect.w - CONTROL_SIZE) > 0.5) problems.push(`collapsed width ${rect.w}`);
  if (rect.w > CONTROL_MAX + 0.5) problems.push(`width ${rect.w} beyond ${CONTROL_MAX}`);
  if (rect.x < row.x - 0.5 || right(rect) > right(row) + 0.5 || rect.y < row.y - 0.5 || bottom(rect) > bottom(row) + 0.5) {
    problems.push(`rect ${JSON.stringify(rect)} outside ${JSON.stringify(row)}`);
  }
  if (menu) {
    if (right(rect) > menu.x + 0.5) problems.push(`ends at ${right(rect)}, past the menu button at ${menu.x}`);
    if (menu.x - right(rect) > 8) problems.push(`leaves ${menu.x - right(rect)} points before the menu button`);
    if (Math.abs(rect.y - menu.y) > 0.5) problems.push(`top ${rect.y}, menu button top ${menu.y}`);
  }
  return problems;
}

/**
 * Problems with where the session menu opened: below the control, or above
 * it from a bottom bar, and inside the window either way.
 */
export function menuProblems(menu: Rect, control: Rect, upward: boolean, window: Rect): string[] {
  const problems: string[] = [];
  if (upward ? bottom(menu) > control.y + 0.5 : menu.y < bottom(control) - 0.5) {
    problems.push(`menu ${JSON.stringify(menu)} is not ${upward ? "above" : "below"} the control ${JSON.stringify(control)}`);
  }
  if (menu.x < window.x - 0.5 || right(menu) > right(window) + 0.5) {
    problems.push(`menu ${JSON.stringify(menu)} leaves the window ${JSON.stringify(window)}`);
  }
  return problems;
}

/** Throws `label` and the problems when there are any. */
export function assertNone(label: string, problems: string[]): void {
  if (problems.length > 0) throw new Error(`${label}: ${problems.join("; ")}`);
}
