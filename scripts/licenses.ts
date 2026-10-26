/**
 * Generate and check `third-party/licenses/licenses.json`, the notices for
 * the open source software Huterm ships. The desktop client's Licenses view
 * reads it, and both platforms' packages carry it.
 */
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
export const artifactPath = "third-party/licenses/licenses.json";
const componentsPath = "third-party/licenses/components.json";
const clarificationsPath = "third-party/licenses/crates.json";
const provenancePaths = ["third-party/licenses/provenance.json", "third-party/ghostty/provenance.json"];

/** The release targets; each shipped component records the ones that carry it. */
export const TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"] as const;
export type Target = typeof TARGETS[number];
/** Features the release packages enable per target. */
const RELEASE_FEATURES: Record<Target, string[]> = {
  "aarch64-apple-darwin": ["macos-updater"],
  "x86_64-apple-darwin": ["macos-updater"],
  "aarch64-unknown-linux-gnu": [],
  "x86_64-unknown-linux-gnu": [],
};
const CONDITIONS = ["macos-updater", "appimage"] as const;
export type Condition = typeof CONDITIONS[number];

/** `OR` alternatives are tried in this order; licenses not listed come last. */
const PREFERENCE = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib"];
/**
 * Licenses whose standard text carries no per-work copyright notice, so a
 * committed canonical copy can stand in when a crate ships no file.
 */
const CANONICAL: Record<string, string> = {
  "Apache-2.0": "third-party/licenses/canonical/Apache-2.0.txt",
  "CC0-1.0": "third-party/licenses/canonical/CC0-1.0.txt",
};

// SPDX expressions ---------------------------------------------------------

export type Expression =
  | { kind: "license"; id: string }
  | { kind: "and" | "or"; operands: Expression[] };

/**
 * Parses an SPDX expression as Cargo manifests write it. A legacy `/` is
 * `OR`, `AND` binds tighter than `OR`, and `WITH` stays part of its license.
 */
export function parseExpression(source: string): Expression {
  const tokens = source.replace(/([()/])/g, " $1 ").split(/\s+/).filter(Boolean);
  let position = 0;
  const peek = () => tokens[position];
  const fail = (reason: string): never => { throw new Error(`cannot parse license expression "${source}": ${reason}`); };
  const operand = (): Expression => {
    const token = tokens[position++];
    if (token === undefined) return fail("unexpected end");
    if (token === "(") {
      const inner = disjunction();
      if (tokens[position++] !== ")") fail("missing )");
      return inner;
    }
    if (["AND", "OR", "WITH", ")", "/"].includes(token)) return fail(`unexpected ${token}`);
    let id = token;
    if (peek() === "WITH") {
      position++;
      const exception = tokens[position++];
      if (!exception) fail("missing exception after WITH");
      id = `${id} WITH ${exception}`;
    }
    return { kind: "license", id };
  };
  const join = (kind: "and" | "or", operands: Expression[]): Expression =>
    operands.length === 1 ? operands[0]! : { kind, operands };
  const conjunction = (): Expression => {
    const operands = [operand()];
    while (peek() === "AND") { position++; operands.push(operand()); }
    return join("and", operands);
  };
  const disjunction = (): Expression => {
    const operands = [conjunction()];
    while (peek() === "OR" || peek() === "/") { position++; operands.push(conjunction()); }
    return join("or", operands);
  };
  const expression = disjunction();
  if (position !== tokens.length) fail(`unexpected ${tokens[position]}`);
  return expression;
}

export function formatExpression(expression: Expression, nested = false): string {
  if (expression.kind === "license") return expression.id;
  const text = expression.operands.map(operand => formatExpression(operand, true)).join(expression.kind === "and" ? " AND " : " OR ");
  return nested ? `(${text})` : text;
}

/** The license a `WITH` exception modifies, which is what its file contains. */
const baseLicense = (id: string) => id.split(" WITH ")[0]!;

function licenseIds(expression: Expression): string[] {
  return expression.kind === "license" ? [baseLicense(expression.id)] : expression.operands.flatMap(licenseIds);
}

// License files --------------------------------------------------------------

export type Text = { title: string; text: string };
export type ClassifiedFile = Text & { licenses: Set<string>; notice: boolean };

const flat = (text: string) => text.replace(/\s+/g, " ");

/** Licenses whose characteristic wording appears in `text`. */
export function detectLicenses(text: string): Set<string> {
  const body = flat(text);
  const found = new Set<string>();
  if (/Permission is hereby granted, free of charge, to any person/i.test(body)) found.add("MIT");
  if (/Apache License,? Version 2\.0/i.test(body)) found.add("Apache-2.0");
  if (/Redistribution and use in source and binary forms/i.test(body)) {
    found.add(/Neither the name|names of (?:its|the) contributors may be used/i.test(body) ? "BSD-3-Clause" : "BSD-2-Clause");
  }
  if (/provided 'as-is', without any express or implied warranty/i.test(body) && /Altered source versions must be plainly marked/i.test(body)) found.add("Zlib");
  if (/Permission to use, copy, modify, and\/or distribute this software for any purpose with or without fee is hereby granted/i.test(body)) {
    found.add(/provided that the above copyright notice and this permission notice appear in all copies/i.test(body) ? "ISC" : "0BSD");
  }
  if (/This is free and unencumbered software released into the public domain/i.test(body)) found.add("Unlicense");
  if (/Boost Software License/i.test(body)) found.add("BSL-1.0");
  if (/Mozilla Public License,? (?:version|v\.?) ?2\.0/i.test(body)) found.add("MPL-2.0");
  if (/UNICODE LICENSE V3|UNICODE, INC\. LICENSE AGREEMENT/i.test(body)) found.add("Unicode-3.0");
  if (/CC0 1\.0 Universal/i.test(body)) found.add("CC0-1.0");
  // An `OR` alternative's GPL text is recognized only so it can be passed over.
  if (/GNU GENERAL PUBLIC LICENSE Version 2/i.test(body)) { found.add("GPL-2.0-only"); found.add("GPL-2.0-or-later"); }
  if (/GNU GENERAL PUBLIC LICENSE Version 3/i.test(body)) { found.add("GPL-3.0-only"); found.add("GPL-3.0-or-later"); }
  return found;
}

const NAME_HINTS: [RegExp, string][] = [
  [/0bsd/, "0BSD"],
  [/mit/, "MIT"],
  [/apache/, "Apache-2.0"],
  [/zlib/, "Zlib"],
  [/unlicense/, "Unlicense"],
  [/boost|bsl/, "BSL-1.0"],
  [/isc/, "ISC"],
  [/mpl/, "MPL-2.0"],
  [/unicode/, "Unicode-3.0"],
  [/cc0/, "CC0-1.0"],
  [/bsd/, "BSD"],
];

/** Whether a crate file holds license terms or notices worth shipping. */
export function isLicenseFile(name: string): boolean {
  // `.spdx` files are SPDX metadata documents, not license texts.
  return /^(?:licen[cs]e|copying|unlicense|notice|copyright)/i.test(name) && !/\.spdx$/i.test(name);
}

/**
 * Identifies the licenses one file grants. The file name decides when it
 * names a license; otherwise the wording does, or the declared license when
 * the crate declares only one. An unrecognized file under several declared
 * licenses, such as a "dual-licensed under" pointer, ships as a notice. A
 * name that contradicts the wording fails.
 */
export function classifyFile(title: string, text: string, declared: string[]): ClassifiedFile {
  const lower = title.toLowerCase().replace(/\.(?:md|txt|rst|markdown)$/, "");
  if (/^(?:notice|copyright)/.test(lower)) return { title, text, licenses: new Set(), notice: true };
  const detected = detectLicenses(text);
  const stem = lower.replace(/^(?:licen[cs]e|copying)s?[-_.]?/, "");
  const hint = stem ? NAME_HINTS.find(([pattern]) => pattern.test(stem))?.[1] : undefined;
  if (hint) {
    const matches = [...detected].filter(id => hint === "BSD" ? id.startsWith("BSD-") : id === hint);
    if (detected.size > 0 && matches.length === 0) {
      throw new Error(`${title} is named for ${hint} but reads as ${[...detected].join(", ")}`);
    }
    if (hint === "BSD") {
      if (matches.length === 0) throw new Error(`${title} names BSD without a recognizable clause count`);
      return { title, text, licenses: new Set(matches), notice: false };
    }
    return { title, text, licenses: new Set([hint]), notice: false };
  }
  if (detected.size > 0) return { title, text, licenses: detected, notice: false };
  const unique = [...new Set(declared)];
  if (unique.length === 1) return { title, text, licenses: new Set(unique), notice: false };
  return { title, text, licenses: new Set(), notice: true };
}

// Selection ------------------------------------------------------------------

export type Resolution = { expression: Expression; texts: Text[]; quality: number };
export type Sources = {
  files: ClassifiedFile[];
  /**
   * The licenses `deny.toml` allows. An alternative outside it never wins an
   * `OR`, so a crate that ships only a GPL text fails instead of being listed
   * under GPL. Absent means every license is allowed.
   */
  allowed?: ReadonlySet<string>;
  /** Upstream texts committed for a crate that ships none, by license. */
  clarified: Map<string, Text[]>;
  canonical: Map<string, Text>;
};

const rank = (id: string) => {
  const index = PREFERENCE.indexOf(baseLicense(id));
  return index === -1 ? PREFERENCE.length : index;
};

function firstId(expression: Expression): string {
  return expression.kind === "license" ? expression.id : firstId(expression.operands[0]!);
}

/**
 * Picks the texts that satisfy `expression`. `AND` needs every operand.
 * `OR` takes the alternative with the best sources, the crate's own files
 * or a clarification before a canonical text, breaking ties by
 * [`PREFERENCE`]. Returns `undefined` when nothing satisfies it.
 */
export function resolveExpression(expression: Expression, sources: Sources): Resolution | undefined {
  if (expression.kind === "license") {
    if (sources.allowed && !sources.allowed.has(expression.id)) return undefined;
    const id = baseLicense(expression.id);
    const own = sources.files.filter(file => !file.notice && file.licenses.has(id));
    if (own.length > 0) return { expression, texts: own.map(({ title, text }) => ({ title, text })), quality: 2 };
    const clarified = sources.clarified.get(id);
    if (clarified) return { expression, texts: clarified, quality: 2 };
    const canonical = sources.canonical.get(id);
    if (canonical) return { expression, texts: [canonical], quality: 1 };
    return undefined;
  }
  const resolved = expression.operands.map(operand => resolveExpression(operand, sources));
  if (expression.kind === "and") {
    if (resolved.some(item => item === undefined)) return undefined;
    const parts = resolved as Resolution[];
    const texts: Text[] = [];
    for (const part of parts) for (const text of part.texts) if (!texts.some(item => item.title === text.title && item.text === text.text)) texts.push(text);
    return { expression: { kind: "and", operands: parts.map(part => part.expression) }, texts, quality: Math.min(...parts.map(part => part.quality)) };
  }
  const options = resolved.filter((item): item is Resolution => item !== undefined);
  options.sort((left, right) => right.quality - left.quality || rank(firstId(left.expression)) - rank(firstId(right.expression)));
  return options[0];
}

export type CrateInput = {
  name: string;
  version: string;
  license: string;
  files: Text[];
  allowed?: ReadonlySet<string>;
  clarified: Map<string, Text[]>;
  canonical: Map<string, Text>;
};

/** The license a crate is used under and the texts that carry it. */
export function crateTexts(input: CrateInput): { used: string; texts: Text[] } {
  const expression = parseExpression(input.license);
  const declared = licenseIds(expression);
  let files: ClassifiedFile[];
  try {
    files = input.files.map(file => classifyFile(file.title, file.text, declared));
  } catch (error) {
    throw new Error(`${input.name} ${input.version}: ${error instanceof Error ? error.message : error}`);
  }
  const resolution = resolveExpression(expression, { files, clarified: input.clarified, canonical: input.canonical, allowed: input.allowed });
  if (!resolution) {
    throw new Error(`${input.name} ${input.version} (${input.license}) ships no text for a license deny.toml allows; add a clarification to ${clarificationsPath}`);
  }
  const used = licenseIds(resolution.expression);
  // Apache-2.0 section 4(d) carries NOTICE files; COPYRIGHT files attribute.
  const notices = files.filter(file => file.notice && (!/^notice/i.test(file.title) || used.includes("Apache-2.0")));
  return { used: formatExpression(resolution.expression), texts: [...resolution.texts, ...notices.map(({ title, text }) => ({ title, text }))] };
}

// Normalization and rendering ----------------------------------------------

/** Normalizes layout, never wording: line endings, tabs, trailing spaces, and blank edges. */
export function normalizeText(text: string): string {
  const lines = text.replace(/^﻿/, "").replace(/\r\n?/g, "\n").replace(/\f/g, "\n").split("\n").map(line => {
    let expanded = "";
    for (const character of line) expanded += character === "\t" ? " ".repeat(8 - (expanded.length % 8)) : character;
    return expanded.trimEnd();
  });
  while (lines.length > 0 && lines[0] === "") lines.shift();
  while (lines.length > 0 && lines.at(-1) === "") lines.pop();
  return lines.join("\n");
}

export type ArtifactText = { title: string; text: string } | { title: string; file: string };
export type Component = {
  id: string;
  name: string;
  version?: string;
  kind: string;
  license: string;
  used?: string;
  url?: string;
  targets: Target[];
  condition?: Condition;
  texts: ({ title: string; text: string } | { title: string; file: string })[];
};
export type Artifact = { schema: 1; components: Component[]; texts: Record<string, string> };

const textKey = (text: string) => createHash("sha256").update(text).digest("hex").slice(0, 16);

const sortKey = (component: Component) => `${component.name.toLowerCase()}\u0000${component.version ?? ""}\u0000${component.id}`;

/**
 * Builds the artifact from components whose texts are inline. Texts are
 * normalized and stored once by digest; components sort by name.
 */
export function buildArtifact(components: (Omit<Component, "texts"> & { texts: ({ title: string; text: string } | { title: string; file: string })[] })[]): Artifact {
  const texts: Record<string, string> = {};
  const built = components.map(component => ({
    ...component,
    texts: component.texts.map(item => {
      if ("file" in item) return item;
      const text = normalizeText(item.text);
      if (!text) throw new Error(`${component.id}: ${item.title} is empty`);
      const key = textKey(text);
      texts[key] = text;
      return { title: item.title, text: key };
    }),
  }));
  const ids = new Set<string>();
  for (const component of built) {
    if (ids.has(component.id)) throw new Error(`duplicate component ${component.id}`);
    ids.add(component.id);
    if (component.targets.length === 0) throw new Error(`${component.id} ships on no target`);
  }
  built.sort((left, right) => (sortKey(left) < sortKey(right) ? -1 : sortKey(left) > sortKey(right) ? 1 : 0));
  const ordered: Record<string, string> = {};
  for (const key of Object.keys(texts).sort()) ordered[key] = texts[key]!;
  return { schema: 1, components: built, texts: ordered };
}

export function renderArtifact(artifact: Artifact): string {
  return `${JSON.stringify(artifact, null, 2)}\n`;
}

/** A short account of how `committed` differs from `generated`, or `undefined` when equal. */
export function describeDrift(committed: string, generated: string): string | undefined {
  if (committed === generated) return undefined;
  let before: Artifact | undefined;
  try { before = JSON.parse(committed) as Artifact; } catch { return `${artifactPath} is not valid JSON`; }
  const after = JSON.parse(generated) as Artifact;
  const index = (artifact: Artifact) => new Map(artifact.components.map(component => [component.id, JSON.stringify(component)]));
  const old = index(before);
  const next = index(after);
  const added = [...next.keys()].filter(id => !old.has(id));
  const removed = [...old.keys()].filter(id => !next.has(id));
  const changed = [...next.keys()].filter(id => old.has(id) && old.get(id) !== next.get(id));
  const lines = [`${artifactPath} is out of date; run mise run licenses:generate`];
  for (const [label, ids] of [["added", added], ["removed", removed], ["changed", changed]] as const) {
    if (ids.length > 0) lines.push(`  ${label}: ${ids.slice(0, 10).join(", ")}${ids.length > 10 ? `, and ${ids.length - 10} more` : ""}`);
  }
  if (lines.length === 1) lines.push("  formatting or shared texts differ");
  return lines.join("\n");
}

// Repository inputs ------------------------------------------------------------

type ComponentSpec = {
  id: string;
  name: string;
  version?: string;
  kind: string;
  license: string;
  url?: string;
  targets?: Target[];
  condition?: Condition;
  files: string[];
};
/**
 * Upstream texts for a crate that ships none. `standard: "authors"` marks a
 * crate whose repository has no license file either: it gets the standard
 * text with its Cargo.toml authors, or `holders` when it lists none, as the
 * copyright holders.
 */
type Clarification = { name: string; version: string; license: string; files?: { path: string; url: string; sha256: string }[]; standard?: "authors"; holders?: string };
type Provenance = Record<string, { url: string; sha256: string; rust?: string }>;
type CargoPackage = {
  id: string;
  name: string;
  version: string;
  license: string | null;
  license_file: string | null;
  manifest_path: string;
  repository: string | null;
  homepage: string | null;
  authors: string[];
};

function run(args: string[]): string {
  const child = Bun.spawnSync(args, { cwd: repoRoot, stdout: "pipe", stderr: "pipe" });
  if (child.exitCode !== 0) throw new Error(`${args.join(" ")} failed (${child.exitCode}): ${child.stderr.toString().trim()}`);
  return child.stdout.toString();
}

function readText(path: string): string {
  const bytes = readFileSync(path);
  try { return new TextDecoder("utf-8", { fatal: true }).decode(bytes); } catch { return new TextDecoder("latin1").decode(bytes); }
}

const sha256 = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

/** Whether `version` satisfies a `^x.y.z` or `=x.y.z` requirement. */
export function versionMatches(requirement: string, version: string): boolean {
  const parse = (value: string) => value.split("+")[0]!.split("-")[0]!.split(".").map(Number);
  const exact = requirement.startsWith("=");
  const wanted = parse(requirement.replace(/^[=^]/, ""));
  const actual = parse(version);
  if (wanted.length !== 3 || actual.length !== 3 || [...wanted, ...actual].some(Number.isNaN)) throw new Error(`unsupported version requirement ${requirement} for ${version}`);
  if (exact) return requirement.slice(1) === version;
  const [major, minor, patch] = wanted as [number, number, number];
  const [actualMajor, actualMinor, actualPatch] = actual as [number, number, number];
  const atLeast = actualMajor > major || (actualMajor === major && (actualMinor > minor || (actualMinor === minor && actualPatch >= patch)));
  if (!atLeast) return false;
  if (major > 0) return actualMajor === major;
  if (minor > 0) return actualMajor === 0 && actualMinor === minor;
  return actualMajor === 0 && actualMinor === 0 && actualPatch === patch;
}

function verifyProvenance(): Provenance {
  const all: Provenance = {};
  for (const path of provenancePaths) {
    const records = JSON.parse(readFileSync(join(repoRoot, path), "utf8")) as Provenance;
    for (const [file, record] of Object.entries(records)) {
      const target = join(dirname(join(repoRoot, path)), file);
      if (sha256(target) !== record.sha256) throw new Error(`${relative(repoRoot, target)} differs from the SHA-256 in ${path}`);
      all[relative(repoRoot, target)] = record;
    }
  }
  return all;
}

function toolchainChannel(): string {
  const channel = /^channel\s*=\s*"([^"]+)"/m.exec(readFileSync(join(repoRoot, "rust-toolchain.toml"), "utf8"))?.[1];
  if (!channel) throw new Error("rust-toolchain.toml pins no channel");
  return channel;
}

function shippedPackages(): { packages: Map<string, CargoPackage>; targets: Map<string, Target[]> } {
  run(["cargo", "fetch", "--locked"]);
  const metadata = JSON.parse(run(["cargo", "metadata", "--locked", "--format-version", "1"])) as { packages: CargoPackage[]; workspace_members: string[] };
  const members = new Set(metadata.workspace_members);
  const packages = new Map<string, CargoPackage>();
  for (const item of metadata.packages) if (!members.has(item.id)) packages.set(`${item.name}@${item.version}`, item);
  const targets = new Map<string, Target[]>();
  for (const target of TARGETS) {
    const features = RELEASE_FEATURES[target];
    const output = run(["cargo", "tree", "--locked", "-p", "huterm", "-e", "normal,no-proc-macro", "--target", target, ...(features.length ? ["--features", features.join(",")] : []), "--prefix", "none", "--format", "{p}"]);
    for (const line of new Set(output.split("\n").map(item => item.replace(/ \(\*\)$/, "").trim()).filter(Boolean))) {
      const [name, version] = line.split(" ");
      if (!name || !version?.startsWith("v")) throw new Error(`unexpected cargo tree line: ${line}`);
      const key = `${name}@${version.slice(1)}`;
      if (!packages.has(key)) continue;
      targets.set(key, [...(targets.get(key) ?? []), target]);
    }
  }
  return { packages, targets };
}

function crateFiles(directory: string): Text[] {
  const files: Text[] = [];
  for (const name of readdirSync(directory).sort()) {
    const path = join(directory, name);
    if (isLicenseFile(name) && statSync(path).isFile()) files.push({ title: name, text: readText(path) });
    if (/^LICENSES$/i.test(name) && statSync(path).isDirectory()) {
      for (const inner of readdirSync(path).sort()) files.push({ title: `${name}/${inner}`, text: readText(join(path, inner)) });
    }
  }
  return files;
}

function resolveVersion(value: string | undefined, packages: Map<string, CargoPackage>): string | undefined {
  if (value === undefined || !value.startsWith("$")) return value;
  switch (value) {
    case "$ghostty": return (JSON.parse(readFileSync(join(repoRoot, "scripts/ghostty-source.json"), "utf8")) as { revision: string }).revision.slice(0, 7);
    case "$zig": return (JSON.parse(readFileSync(join(repoRoot, "scripts/ghostty-source.json"), "utf8")) as { zig: string }).zig;
    case "$sparkle": return (JSON.parse(readFileSync(join(repoRoot, "scripts/sparkle-source.json"), "utf8")) as { version: string }).version;
    case "$rust": return toolchainChannel();
    case "$freetype": {
      const header = readFileSync(join(crateDirectory("freetype-sys", packages), "freetype2/include/freetype/freetype.h"), "utf8");
      const part = (name: string) => new RegExp(`#define FREETYPE_${name}\\s+(\\d+)`).exec(header)?.[1];
      const [major, minor, patch] = [part("MAJOR"), part("MINOR"), part("PATCH")];
      if (!major || !minor || !patch) throw new Error("cannot read the bundled FreeType version");
      return `${major}.${minor}.${patch}`;
    }
    default: throw new Error(`unknown version source ${value}`);
  }
}

/** The standard text of `license` naming `item`'s authors, or `override`, as copyright holders. */
function standardText(license: string, item: CargoPackage, override?: string): Text {
  if (license !== "MIT") throw new Error(`${item.name}: no standard template for ${license}`);
  const holders = override ? [override] : item.authors.map(author => author.replace(/\s*<[^>]*>/, "").trim()).filter(Boolean);
  if (holders.length === 0) throw new Error(`${item.name} ${item.version} lists no authors to name as copyright holders`);
  const template = readText(join(repoRoot, "third-party/licenses/canonical/MIT.txt"));
  if (!template.includes("<year> <copyright holders>")) throw new Error("canonical/MIT.txt lacks its copyright placeholder");
  return { title: "MIT standard text, copyright holders from Cargo.toml authors", text: template.replace("<year> <copyright holders>", holders.join(", ")) };
}

function crateDirectory(name: string, packages: Map<string, CargoPackage>): string {
  const matches = [...packages.values()].filter(item => item.name === name);
  if (matches.length !== 1) throw new Error(`expected one ${name} package, found ${matches.length}`);
  return dirname(matches[0]!.manifest_path);
}

/** Splits an optional `#L<first>-L<last>` line range off a file reference. */
export function parseFileReference(reference: string): { path: string; lines?: [number, number] } {
  const match = /^(.*)#L(\d+)-L(\d+)$/.exec(reference);
  if (!match) return { path: reference };
  const first = Number(match[2]);
  const last = Number(match[3]);
  if (first < 1 || last < first) throw new Error(`invalid line range in ${reference}`);
  return { path: match[1]!, lines: [first, last] };
}

/** Lines `first` through `last` of `text`, one-based and inclusive. */
export function selectLines(text: string, lines?: [number, number]): string {
  if (!lines) return text;
  const all = text.split(/\r?\n/);
  if (lines[1] > all.length) throw new Error(`line range ${lines[0]}-${lines[1]} passes the end of a ${all.length}-line file`);
  return all.slice(lines[0] - 1, lines[1]).join("\n");
}

/** Generates the artifact from the lockfile, crate sources, and committed notices. */
export function generate(): string {
  const provenance = verifyProvenance();
  const channel = toolchainChannel();
  for (const [file, record] of Object.entries(provenance)) {
    if (record.rust !== undefined && record.rust !== channel) throw new Error(`${file} comes from Rust ${record.rust}, but rust-toolchain.toml pins ${channel}; refresh it`);
  }
  const { packages, targets } = shippedPackages();
  const allowed = new Set((Bun.TOML.parse(readFileSync(join(repoRoot, "deny.toml"), "utf8")) as { licenses: { allow: string[] } }).licenses.allow);
  const policy = JSON.parse(readFileSync(join(repoRoot, "assets/linux/package-policy.json"), "utf8")) as { privateLibraries: Record<string, { licenseFile: string }> };
  const packagedFiles = new Set(Object.values(policy.privateLibraries).map(library => library.licenseFile));
  const canonical = new Map(Object.entries(CANONICAL).map(([id, path]) => [id, { title: `${id} standard text`, text: readText(join(repoRoot, path)) }]));
  const clarifications = (JSON.parse(readFileSync(join(repoRoot, clarificationsPath), "utf8")) as { crates: Clarification[] }).crates;
  const usedClarifications = new Set<Clarification>();
  const components: Parameters<typeof buildArtifact>[0] = [];
  const failures: string[] = [];
  for (const [key, shippedOn] of targets) {
    try {
    const item = packages.get(key)!;
    const directory = dirname(item.manifest_path);
    const files = crateFiles(directory);
    let license = item.license;
    if (!license) {
      if (!item.license_file) throw new Error(`${key} declares no license`);
      const declared = detectLicenses(readText(join(directory, item.license_file)));
      if (declared.size === 0) throw new Error(`${key} declares only ${item.license_file}, whose license is unrecognizable`);
      license = [...declared].join(" AND ");
    }
    const clarified = new Map<string, Text[]>();
    for (const clarification of clarifications) {
      if (clarification.name !== item.name || !versionMatches(clarification.version, item.version)) continue;
      usedClarifications.add(clarification);
      if (clarification.standard === "authors") {
        clarified.set(clarification.license, [standardText(clarification.license, item, clarification.holders)]);
        continue;
      }
      clarified.set(clarification.license, (clarification.files ?? []).map(file => {
        if (sha256(join(repoRoot, file.path)) !== file.sha256) throw new Error(`${file.path} differs from the SHA-256 in ${clarificationsPath}`);
        return { title: file.path.split("/").at(-1)!, text: readText(join(repoRoot, file.path)) };
      }));
    }
    const { used, texts } = crateTexts({ name: item.name, version: item.version, license, files, clarified, canonical, allowed });
    components.push({
      id: key,
      name: item.name,
      version: item.version,
      kind: "Rust crate",
      license,
      ...(formatExpression(parseExpression(license)) === used ? {} : { used }),
      url: item.repository ?? item.homepage ?? `https://crates.io/crates/${item.name}`,
      targets: TARGETS.filter(target => shippedOn.includes(target)),
      texts,
    });
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
  }
  if (failures.length > 0) throw new Error(failures.join("\n"));
  const stale = clarifications.filter(clarification => !usedClarifications.has(clarification));
  if (stale.length > 0) throw new Error(`unused clarifications in ${clarificationsPath}: ${stale.map(item => `${item.name} ${item.version}`).join(", ")}`);
  const specs = (JSON.parse(readFileSync(join(repoRoot, componentsPath), "utf8")) as { components: ComponentSpec[] }).components;
  for (const spec of specs) {
    if (spec.condition !== undefined && !CONDITIONS.includes(spec.condition)) throw new Error(`${spec.id} has unknown condition ${spec.condition}`);
    const version = resolveVersion(spec.version, packages);
    components.push({
      id: spec.id,
      name: spec.name,
      ...(version === undefined ? {} : { version }),
      kind: spec.kind,
      license: spec.license,
      ...(spec.url === undefined ? {} : { url: spec.url }),
      targets: spec.targets ?? [...TARGETS],
      ...(spec.condition === undefined ? {} : { condition: spec.condition }),
      texts: spec.files.map(file => {
        if (file.startsWith("packaged:")) {
          const name = file.slice("packaged:".length);
          if (!packagedFiles.has(name)) throw new Error(`${spec.id} lists packaged:${name}, which assets/linux/package-policy.json does not copy`);
          return { title: name, file: name };
        }
        if (file.startsWith("crate:")) {
          const { path: relativePath, lines } = parseFileReference(file.slice("crate:".length));
          const [crate, ...rest] = relativePath.split("/");
          const text = selectLines(readText(join(crateDirectory(crate!, packages), ...rest)), lines);
          // A dependency bump can move a header; a range must still hold a notice.
          if (lines && !/copyright/i.test(text)) throw new Error(`${spec.id}: ${file} no longer selects a copyright notice`);
          return { title: lines ? `${rest.join("/")} (lines ${lines[0]}-${lines[1]})` : rest.join("/"), text };
        }
        if (!existsSync(join(repoRoot, file))) throw new Error(`${spec.id} lists missing ${file}`);
        return { title: file.split("/").at(-1)!, text: readText(join(repoRoot, file)) };
      }),
    });
  }
  return renderArtifact(buildArtifact(components));
}

if (import.meta.main) {
  const command = Bun.argv[2];
  try {
    if (command === "generate") {
      writeFileSync(join(repoRoot, artifactPath), generate());
      console.log(`wrote ${artifactPath}`);
    } else if (command === "check") {
      const drift = describeDrift(readFileSync(join(repoRoot, artifactPath), "utf8"), generate());
      if (drift) throw new Error(drift);
      console.log(`${artifactPath} is current`);
    } else {
      throw new Error("usage: bun scripts/licenses.ts generate|check");
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
