import { describe, expect, test } from "bun:test";
import {
  buildArtifact,
  classifyFile,
  crateTexts,
  describeDrift,
  formatExpression,
  normalizeText,
  parseExpression,
  renderArtifact,
  versionMatches,
  type Text,
} from "./licenses";

const MIT = "MIT License\n\nCopyright (c) 2024 Example\n\nPermission is hereby granted, free of charge, to any person obtaining a copy";
const APACHE = "Apache License\nVersion 2.0, January 2004\nTERMS AND CONDITIONS";
const BSD3 = "Redistribution and use in source and binary forms, with or without modification. Neither the name of the copyright holder";
const CANONICAL_APACHE: Text = { title: "Apache-2.0 standard text", text: APACHE };

function texts(license: string, files: Text[], clarified = new Map<string, Text[]>()) {
  return crateTexts({ name: "demo", version: "1.0.0", license, files, clarified, canonical: new Map([["Apache-2.0", CANONICAL_APACHE]]) });
}

describe("parseExpression", () => {
  test("reads legacy slashes as OR, binds AND tighter, and keeps WITH on its license", () => {
    expect(formatExpression(parseExpression("MIT/Apache-2.0"))).toBe("MIT OR Apache-2.0");
    expect(parseExpression("MIT OR Apache-2.0 AND BSD-3-Clause")).toEqual({
      kind: "or",
      operands: [{ kind: "license", id: "MIT" }, { kind: "and", operands: [{ kind: "license", id: "Apache-2.0" }, { kind: "license", id: "BSD-3-Clause" }] }],
    });
    expect(formatExpression(parseExpression("(Apache-2.0 WITH LLVM-exception OR MIT) AND Unicode-3.0"))).toBe("(Apache-2.0 WITH LLVM-exception OR MIT) AND Unicode-3.0");
  });

  test("rejects malformed expressions", () => {
    expect(() => parseExpression("MIT OR")).toThrow("unexpected end");
    expect(() => parseExpression("(MIT")).toThrow("missing )");
    expect(() => parseExpression("MIT Apache-2.0")).toThrow("unexpected Apache-2.0");
  });
});

describe("classifyFile", () => {
  test("trusts license-named files, separates 0BSD from BSD, and fails on contradictions", () => {
    expect([...classifyFile("LICENSE-MIT", MIT, ["MIT", "Apache-2.0"]).licenses]).toEqual(["MIT"]);
    expect([...classifyFile("LICENSE-0BSD", "Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted.", ["0BSD"]).licenses]).toEqual(["0BSD"]);
    expect([...classifyFile("LICENSE-BSD", BSD3, ["BSD-3-Clause"]).licenses]).toEqual(["BSD-3-Clause"]);
    expect(() => classifyFile("LICENSE-MIT", APACHE, ["MIT", "Apache-2.0"])).toThrow("named for MIT but reads as Apache-2.0");
  });

  test("reads generic files by wording, then by a lone declared license, else as a notice", () => {
    expect([...classifyFile("LICENSE.txt", `${MIT}\n\n${APACHE}`, ["MIT", "Apache-2.0"]).licenses].sort()).toEqual(["Apache-2.0", "MIT"]);
    expect([...classifyFile("LICENSE", "Copyright 2020 Someone. All rights granted.", ["MIT"]).licenses]).toEqual(["MIT"]);
    const pointer = classifyFile("COPYING", "This project is dual-licensed under the Unlicense and MIT licenses.", ["Unlicense", "MIT"]);
    expect(pointer.notice).toBe(true);
    expect(pointer.licenses.size).toBe(0);
  });
});

describe("crateTexts", () => {
  test("prefers the crate's own MIT text, then a canonical Apache-2.0 text", () => {
    expect(texts("MIT OR Apache-2.0", [{ title: "LICENSE-APACHE", text: APACHE }, { title: "LICENSE-MIT", text: MIT }])).toEqual({ used: "MIT", texts: [{ title: "LICENSE-MIT", text: MIT }] });
    expect(texts("Zlib OR Apache-2.0 OR MIT", [])).toEqual({ used: "Apache-2.0", texts: [CANONICAL_APACHE] });
  });

  test("passes over an unwanted alternative and keeps every AND operand", () => {
    const gpl = { title: "LICENSE-GPLv2", text: "GNU GENERAL PUBLIC LICENSE\nVersion 2, June 1991" };
    expect(texts("Apache-2.0 OR GPL-2.0-only", [gpl, { title: "LICENSE-APACHE", text: APACHE }]).used).toBe("Apache-2.0");
    const result = texts("(Apache-2.0 OR MIT) AND BSD-3-Clause", [{ title: "LICENSE-MIT", text: MIT }, { title: "LICENSE-WHATWG", text: BSD3 }]);
    expect(result.used).toBe("MIT AND BSD-3-Clause");
    expect(result.texts.map(text => text.title)).toEqual(["LICENSE-MIT", "LICENSE-WHATWG"]);
  });

  test("carries NOTICE files only for Apache-2.0 and COPYRIGHT files always", () => {
    const notice = { title: "NOTICE", text: "Example NOTICE" };
    const copyright = { title: "COPYRIGHT", text: "Copyright Example" };
    expect(texts("Apache-2.0", [{ title: "LICENSE", text: APACHE }, notice]).texts.map(text => text.title)).toEqual(["LICENSE", "NOTICE"]);
    expect(texts("MIT", [{ title: "LICENSE", text: MIT }, notice, copyright]).texts.map(text => text.title)).toEqual(["LICENSE", "COPYRIGHT"]);
  });

  test("uses a clarification and fails naming the crate when nothing grants its license", () => {
    const upstream = [{ title: "LICENSE", text: MIT }];
    expect(texts("MIT", [], new Map([["MIT", upstream]])).texts).toEqual(upstream);
    expect(() => texts("MIT", [])).toThrow("demo 1.0.0 (MIT) ships no license text Huterm can use; add a clarification");
  });
});

test("normalizeText changes layout only", () => {
  expect(normalizeText("﻿\r\n\r\nline one  \r\n\tindented\r\na\tb\n\n")).toBe("line one\n        indented\na       b");
});

test("versionMatches follows Cargo caret ranges below and above 1.0", () => {
  expect(versionMatches("^0.6.5", "0.6.9")).toBe(true);
  expect(versionMatches("^0.6.5", "0.6.4")).toBe(false);
  expect(versionMatches("^0.6.5", "0.7.0")).toBe(false);
  expect(versionMatches("^0.0.6", "0.0.7")).toBe(false);
  expect(versionMatches("^4.1.0", "4.9.0")).toBe(true);
  expect(versionMatches("^4.1.0", "5.0.0")).toBe(false);
  expect(versionMatches("=1.2.3", "1.2.4")).toBe(false);
});

describe("buildArtifact", () => {
  const component = (id: string, name: string, text: string) => ({
    id, name, kind: "Rust crate", license: "MIT", targets: ["x86_64-unknown-linux-gnu" as const], texts: [{ title: "LICENSE", text }],
  });

  test("stores each normalized text once and sorts components by name", () => {
    const artifact = buildArtifact([component("zeta@1", "zeta", `${MIT}\r\n`), component("Alpha@1", "Alpha", MIT)]);
    expect(artifact.components.map(item => item.id)).toEqual(["Alpha@1", "zeta@1"]);
    expect(Object.keys(artifact.texts)).toHaveLength(1);
    expect(artifact.components[0]!.texts[0]).toEqual(artifact.components[1]!.texts[0]!);
  });

  test("rejects duplicate ids and components that ship nowhere", () => {
    expect(() => buildArtifact([component("a@1", "a", MIT), component("a@1", "a", MIT)])).toThrow("duplicate component a@1");
    expect(() => buildArtifact([{ ...component("a@1", "a", MIT), targets: [] }])).toThrow("ships on no target");
  });
});

test("describeDrift reports nothing for a current artifact and names changed components", () => {
  const before = renderArtifact(buildArtifact([
    { id: "a@1", name: "a", kind: "Rust crate", license: "MIT", targets: ["x86_64-unknown-linux-gnu"], texts: [{ title: "LICENSE", text: MIT }] },
  ]));
  const after = renderArtifact(buildArtifact([
    { id: "a@1", name: "a", kind: "Rust crate", license: "MIT", targets: ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"], texts: [{ title: "LICENSE", text: MIT }] },
    { id: "b@2", name: "b", kind: "Rust crate", license: "MIT", targets: ["x86_64-unknown-linux-gnu"], texts: [{ title: "LICENSE", text: MIT }] },
  ]));
  expect(describeDrift(before, before)).toBeUndefined();
  expect(describeDrift(before, after)).toBe("third-party/licenses/licenses.json is out of date; run mise run licenses:generate\n  added: b@2\n  changed: a@1");
  expect(describeDrift("{", after)).toBe("third-party/licenses/licenses.json is not valid JSON");
});
