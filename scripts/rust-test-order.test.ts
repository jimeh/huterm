import { expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkTree, findViolations } from "./rust-test-order.ts";

const lines = (source: string) => findViolations(source).map((violation) => violation.line);

test("test modules after production items pass", () => {
  expect(findViolations(`
fn production() {}

#[cfg(test)]
mod tests {
    fn helper() {}
}

#[cfg(test)]
mod other_tests {}
`)).toEqual([]);
});

test("a production fn after an inline tests module fails", () => {
  const violations = findViolations(`
#[cfg(test)]
mod tests {
    #[test]
    fn works() {}
}

pub(crate) fn production() {}
`);
  expect(violations).toEqual([
    { line: 8, message: "pub(crate) fn production follows test module mod tests (line 3); move the test module below production items" },
  ]);
});

test("a named test module followed by production fails", () => {
  expect(lines(`
#[cfg(test)]
mod update_tests {}

/// Documented production trait.
pub(super) trait Extension {
    fn matches(self) -> bool;
}
impl Extension for u8 {
    fn matches(self) -> bool { true }
}
`)).toEqual([6, 9]);
});

test("several test modules followed by production report the production item", () => {
  const violations = findViolations(`
#[cfg(test)]
mod first_tests {}
#[cfg(test)]
mod second_tests {}
struct Production;
`);
  expect(violations).toEqual([
    { line: 6, message: "struct Production follows test module mod second_tests (line 5); move the test module below production items" },
  ]);
});

test("violations inside nested production modules are reported", () => {
  expect(lines(`
mod outer {
    pub mod inner {
        #[cfg(test)]
        mod tests {}
        fn production() {}
    }
    fn fine() {}
}
`)).toEqual([6]);
});

test("out-of-line test modules do not start the rule", () => {
  expect(findViolations(`
#[cfg(test)]
mod tests;

fn production() {}
`)).toEqual([]);
});

test("only test-only cfg predicates count as test modules", () => {
  expect(findViolations(`
#[cfg(any(test, feature = "x"))]
mod shared {}
#[cfg(not(test))]
mod production_only {}
#[cfg_attr(test, derive(Debug))]
mod attributed {}
fn production() {}
`)).toEqual([]);
  expect(lines(`
#[cfg(all(test, unix))]
mod unix_tests {}
fn production() {}
`)).toEqual([4]);
  expect(lines(`
#[cfg(all(unix, any(test, doc)))]
mod not_test_only {}
#[cfg(all(unix, all(test)))]
mod nested_tests {}
fn production() {}
`)).toEqual([6]);
});

test("test-only items after a test module are allowed", () => {
  expect(findViolations(`
#[cfg(test)]
mod tests {}
#[cfg(test)]
fn helper() {}
#[cfg(test)]
use std::fmt;
#[cfg(test)]
#[path = "fixture.rs"]
mod fixture;
#[cfg(all(test, target_os = "macos"))]
mod macos_tests {}
`)).toEqual([]);
});

test("delimiters inside literals, comments, and lifetimes are not structure", () => {
  expect(lines(`
#![allow(dead_code)]
//! Inner docs with } and {
/* outer /* nested } */ still { comment */
fn literals<'a>(value: &'a str) -> &'static str {
    let _ = ("}", "\\"}", b"}", c"{", r"}", r#"}"#, br##"}"#{"##, cr#"{"#);
    let _ = ('{', '}', '\\'', b'}', '\\u{7b}', 'λ', '"');
    'outer: loop { break 'outer; }
    let r#mod = value;
    r#mod
}
#[cfg(test)]
mod tests {
    const BRACE: char = '}';
    /// Docs with {
    fn helper() -> &'static str { "{" }
}
fn after() {}
`)).toEqual([18]);
});

test("const, static, use, macro, and macro_rules items each count once", () => {
  const violations = findViolations(`
#[cfg(test)]
mod tests {}
const X: S = S { a: 1 };
static Y: S = S { a: 2 };
pub const fn constant() -> u8 { 3 }
use std::{fmt, io};
thread_local! { static Z: u8 = 0; }
lazy!(name, { 1 });
macro_rules! rules { () => {}; }
extern crate core;
extern "C" { fn native(); }
type Alias = [u8; { 4 }];
`);
  expect(violations.map((violation) => [violation.line, violation.message.split(" follows ")[0]])).toEqual([
    [4, "const X: S"],
    [5, "static Y: S"],
    [6, "pub const fn constant"],
    [7, "use std::{fmt, io}"],
    [8, "thread_local!"],
    [9, "lazy!"],
    [10, "macro_rules! rules"],
    [11, "extern crate core"],
    [12, "extern \"C\""],
    [13, "type Alias"],
  ]);
});

test("fn and impl bodies are not searched for module items", () => {
  expect(findViolations(`
fn body() {
    #[cfg(test)]
    mod inner {}
    fn nested() {}
}
impl Type {
    #[cfg(test)]
    const X: u8 = 1;
    fn method() {}
}
`)).toEqual([]);
});

test("unterminated literals are reported as errors", () => {
  expect(() => findViolations(`fn broken() { let _ = "open; }`)).toThrow("unterminated string");
  expect(() => findViolations(`/* open`)).toThrow("unterminated block comment");
});

test("tree walk skips generated and hidden directories", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-test-order-"));
  try {
    const bad = "#[cfg(test)]\nmod tests {}\nfn production() {}\n";
    await mkdir(join(root, "crates", "a", "src"), { recursive: true });
    await writeFile(join(root, "crates", "a", "src", "lib.rs"), bad);
    await writeFile(join(root, "crates", "a", "src", "ok.rs"), "fn production() {}\n");
    for (const skipped of ["target", ".native", "node_modules", "dist", "third-party", ".hidden"]) {
      await mkdir(join(root, skipped), { recursive: true });
      await writeFile(join(root, skipped, "lib.rs"), bad);
    }
    const result = checkTree(root);
    expect(result.files).toBe(2);
    expect(result.reports).toEqual([
      "crates/a/src/lib.rs:3: fn production follows test module mod tests (line 2); move the test module below production items",
    ]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
