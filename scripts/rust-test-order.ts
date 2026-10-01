/**
 * Check that inline Rust test modules follow every production item in their
 * module. Once an inline module gated by a test-only `#[cfg]` appears in an
 * item list, each later item in that list must also be test-only. Clippy's
 * `items_after_test_module` only covers a module named exactly `tests`.
 *
 * Known limitations: items are recognised from tokens, not a full Rust parser.
 * A brace group before an item's body, such as a const-generic default
 * `fn f<const N: usize = { 1 }>()`, ends the item early. Doc comments are
 * skipped like other comments, which is safe because they cannot gate items.
 */
import { readdirSync, readFileSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";

export interface Violation { line: number; message: string }

type Kind = "ident" | "punct" | "literal" | "lifetime";
interface Token { kind: Kind; text: string; line: number; start: number; end: number }

const identStart = /[\p{L}_]/u;
const identPart = /[\p{L}\p{N}_]/u;
const openers: Record<string, string> = { "(": ")", "[": "]", "{": "}" };

function tokenize(source: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  let line = 1;
  const fail = (what: string, at: number): never => { throw new Error(`line ${at}: unterminated ${what}`); };
  // Advance over source[from, to), counting newlines.
  const skip = (to: number) => {
    for (; i < to; i++) if (source[i] === "\n") line++;
  };
  const push = (kind: Kind, start: number, startLine: number) => {
    tokens.push({ kind, text: source.slice(start, i), line: startLine, start, end: i });
  };
  const quoted = (quote: string, what: string) => {
    const startLine = line;
    i++;
    while (source[i] !== quote) {
      if (i >= source.length) fail(what, startLine);
      skip(i + (source[i] === "\\" ? 2 : 1));
    }
    i++;
  };

  while (i < source.length) {
    const c = source[i]!;
    const start = i;
    const startLine = line;
    if (/\s/.test(c)) { skip(i + 1); continue; }
    if (source.startsWith("//", i)) {
      const newline = source.indexOf("\n", i);
      i = newline === -1 ? source.length : newline;
      continue;
    }
    if (source.startsWith("/*", i)) {
      let depth = 0;
      do {
        if (i >= source.length) fail("block comment", startLine);
        if (source.startsWith("/*", i)) { depth++; i += 2; }
        else if (source.startsWith("*/", i)) { depth--; i += 2; }
        else skip(i + 1);
      } while (depth > 0);
      continue;
    }
    const raw = /^[bc]?r(#*)"/.exec(source.slice(i, i + 260));
    if (raw) {
      const closing = `"${raw[1]}`;
      const end = source.indexOf(closing, i + raw[0].length);
      if (end === -1) fail("raw string", startLine);
      skip(end + closing.length);
      push("literal", start, startLine);
      continue;
    }
    if ((c === "b" || c === "c") && source[i + 1] === '"') {
      i++;
      quoted('"', "string");
      push("literal", start, startLine);
      continue;
    }
    if (c === "r" && source[i + 1] === "#" && identStart.test(source[i + 2] ?? "")) {
      i += 2;
      while (i < source.length && identPart.test(source[i]!)) i++;
      push("ident", start, startLine);
      continue;
    }
    if (c === '"') {
      quoted('"', "string");
      push("literal", start, startLine);
      continue;
    }
    if (c === "'" || (c === "b" && source[i + 1] === "'")) {
      const quote = c === "b" ? i + 1 : i;
      const next = source.codePointAt(quote + 1) ?? 0;
      const width = next > 0xffff ? 2 : 1;
      if (source[quote + 1] === "\\" || source[quote + 1 + width] === "'") {
        i = quote;
        quoted("'", "character literal");
        push("literal", start, startLine);
        continue;
      }
      if (c === "'") {
        i++;
        while (i < source.length && identPart.test(source[i]!)) i++;
        push("lifetime", start, startLine);
        continue;
      }
    }
    if (identPart.test(c)) {
      while (i < source.length && identPart.test(source[i]!)) i++;
      push(identStart.test(c) ? "ident" : "literal", start, startLine);
      continue;
    }
    i++;
    push("punct", start, startLine);
  }
  return tokens;
}

/** Map each opening delimiter's index to its closing delimiter's index. */
function matchDelimiters(tokens: Token[]): Map<number, number> {
  const matches = new Map<number, number>();
  const stack: number[] = [];
  tokens.forEach((token, index) => {
    if (token.kind !== "punct") return;
    if (openers[token.text]) stack.push(index);
    else if (Object.values(openers).includes(token.text)) {
      const open = stack.pop();
      if (open === undefined || openers[tokens[open]!.text] !== token.text) {
        throw new Error(`line ${token.line}: unbalanced ${token.text}`);
      }
      matches.set(open, index);
    }
  });
  if (stack.length) throw new Error(`line ${tokens[stack.at(-1)!]!.line}: unclosed ${tokens[stack.at(-1)!]!.text}`);
  return matches;
}

interface Predicate { name: string; args?: Predicate[] }

function parsePredicates(tokens: Token[], from: number, to: number): Predicate[] {
  const predicates: Predicate[] = [];
  let i = from;
  while (i < to) {
    const name = tokens[i]!.text;
    i++;
    if (tokens[i]?.text === "=") i += 2;
    let args: Predicate[] | undefined;
    if (tokens[i]?.text === "(") {
      let depth = 0;
      let close = i;
      for (; close < to; close++) {
        if (tokens[close]!.text === "(") depth++;
        else if (tokens[close]!.text === ")" && --depth === 0) break;
      }
      args = parsePredicates(tokens, i + 1, close);
      i = close + 1;
    }
    predicates.push({ name, args });
    if (tokens[i]?.text === ",") i++;
  }
  return predicates;
}

function testOnly(predicate: Predicate): boolean {
  if (predicate.name === "test") return predicate.args === undefined;
  return predicate.name === "all" && (predicate.args ?? []).some(testOnly);
}

/** True when an attribute's tokens are `cfg(<test-only predicate>)`. */
function isTestCfg(attribute: Token[]): boolean {
  if (attribute[0]?.text !== "cfg" || attribute[1]?.text !== "(" || attribute.at(-1)?.text !== ")") return false;
  const predicates = parsePredicates(attribute, 2, attribute.length - 1);
  return predicates.length === 1 && testOnly(predicates[0]!);
}

interface Item { line: number; label: string; testOnly: boolean; body?: [number, number] }

const semicolonItems = new Set(["use", "type", "static"]);
const fnQualifiers = new Set(["fn", "unsafe", "async", "extern"]);
const labelStops = new Set(["{", ";", "=", "(", "where"]);

export function findViolations(source: string): Violation[] {
  const tokens = tokenize(source);
  const matches = matchDelimiters(tokens);
  const text = (index: number) => tokens[index]?.text;
  const violations: Violation[] = [];

  /** Describe an item by its source up to its first body, parameter list, or initialiser. */
  const label = (start: number, keyword: number, limit: number, stops: Set<string>) => {
    let end = keyword;
    while (end < limit && !stops.has(text(end)!)) end = (matches.get(end) ?? end) + 1;
    return source.slice(tokens[start]!.start, tokens[Math.max(end - 1, start)]!.end).replace(/\s+/g, " ").slice(0, 80);
  };

  const parseItem = (start: number, attributes: Token[][], limit: number): [Item, number] => {
    let keyword = start;
    if (text(start) === "pub") keyword = text(start + 1) === "(" ? matches.get(start + 1)! + 1 : start + 1;
    const first = text(keyword)!;
    const semicolonOnly = semicolonItems.has(first)
      || (first === "const" && !fnQualifiers.has(text(keyword + 1)!))
      || (first === "extern" && text(keyword + 1) === "crate");
    const item: Item = {
      line: tokens[start]!.line,
      label: label(start, keyword, limit, first === "use" ? new Set([";"]) : labelStops),
      testOnly: attributes.some(isTestCfg),
    };
    for (let i = keyword; i < limit; i = (matches.get(i) ?? i) + 1) {
      if (text(i) === ";") return [item, i + 1];
      if (text(i) === "{" && !semicolonOnly) {
        if (first === "mod") item.body = [i + 1, matches.get(i)!];
        return [item, matches.get(i)! + 1];
      }
    }
    throw new Error(`line ${item.line}: item has no terminating ';' or '}'`);
  };

  const parseItems = (from: number, to: number) => {
    let testModule: Item | undefined;
    let i = from;
    while (i < to) {
      if (text(i) === "#" && text(i + 1) === "!" && text(i + 2) === "[") { i = matches.get(i + 2)! + 1; continue; }
      if (text(i) === ";") { i++; continue; }
      const attributes: Token[][] = [];
      while (text(i) === "#" && text(i + 1) === "[") {
        attributes.push(tokens.slice(i + 2, matches.get(i + 1)!));
        i = matches.get(i + 1)! + 1;
      }
      const [item, next] = parseItem(i, attributes, to);
      if (testModule && !item.testOnly) {
        violations.push({
          line: item.line,
          message: `${item.label} follows test module ${testModule.label} (line ${testModule.line}); move the test module below production items`,
        });
      }
      if (item.body && item.testOnly) testModule = item;
      else if (item.body) parseItems(item.body[0], item.body[1]);
      i = next;
    }
  };

  parseItems(0, tokens.length);
  return violations.sort((a, b) => a.line - b.line);
}

const skippedDirectories = new Set(["target", "node_modules", "dist", "third-party"]);

function rustFiles(directory: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!entry.name.startsWith(".") && !skippedDirectories.has(entry.name)) files.push(...rustFiles(path));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      files.push(path);
    }
  }
  return files;
}

/** Check every Rust file under `root`, returning `path:line: message` reports. */
export function checkTree(root: string): { files: number; reports: string[] } {
  const files = rustFiles(root);
  const reports: string[] = [];
  for (const file of files) {
    const path = relative(root, file).split(sep).join("/");
    try {
      for (const violation of findViolations(readFileSync(file, "utf8"))) reports.push(`${path}:${violation.line}: ${violation.message}`);
    } catch (error) {
      reports.push(`${path}: ${(error as Error).message}`);
    }
  }
  return { files: files.length, reports };
}

if (import.meta.main) {
  const root = resolve(Bun.argv[2] ?? join(import.meta.dir, ".."));
  const { files, reports } = checkTree(root);
  for (const report of reports) console.error(report);
  if (reports.length) {
    console.error(`rust-test-order: ${reports.length} problem(s) in ${files} Rust files`);
    process.exit(1);
  }
  console.log(`rust-test-order: checked ${files} Rust files`);
}
