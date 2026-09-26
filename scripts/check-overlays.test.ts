import { expect, test } from "bun:test";
import { field, parseRect, scrollPillCentre } from "./check-overlays";

const state = `windows=1 config.warning=None desktop.notices=0
w0.palette=false w0.tabs=3 w0.text="READY busy"
w0.menu=true w0.menu_button=260,4,26,26 w0.dialog_title="Close 2 tabs?" w0.tabs_rects=1,0,96,34;97,0,96,34 w0.terminal_bounds=0,34,1208,520 w0.scrolled=12
`;

test("fields are read by exact name, so prefixes do not alias", () => {
  expect(field(state, "menu")).toBe("true");
  expect(field(state, "menu_button")).toBe("260,4,26,26");
  expect(field(state, "tabs")).toBe("3");
  expect(field(state, "tabs_rects")).toBe("1,0,96,34;97,0,96,34");
  expect(field(state, "scrolled")).toBe("12");
  expect(() => field(state, "missing")).toThrow("missing w0.missing");
});

test("rects parse four finite numbers and reject anything else", () => {
  expect(parseRect("0,34,1208,520")).toEqual({ x: 0, y: 34, w: 1208, h: 520 });
  expect(() => parseRect("none")).toThrow("invalid rect");
  expect(() => parseRect("1,2,3")).toThrow("invalid rect");
});

test("the scroll pill centre sits at the terminal's bottom centre above the 12-point margin", () => {
  expect(scrollPillCentre({ x: 0, y: 34, w: 1208, h: 520 })).toEqual({ x: 604, y: 527 });
});
