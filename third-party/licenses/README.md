# Open source license notices

`licenses.json` lists every open source component Huterm ships, with its
license and full notice texts. The About panel's Licenses view reads it, and
`Huterm.app` and both Linux packages carry it. `mise run licenses:generate`
writes it; `mise run licenses:check`, part of `mise run verify` and CI's Policy
job, fails when it is out of date. Regenerate it after any dependency,
toolchain, or notice change.

`scripts/licenses.ts` builds it from these inputs:

- Rust crates in the release graph of each release target, from `cargo tree
  -e normal,no-proc-macro`. Build-only and proc-macro crates do not ship.
  Workspace crates are Huterm's own; vendored patched crates are included.
- Each crate's own license files. For an `OR` expression the generator takes
  the alternative the crate ships a text for, preferring MIT, then
  Apache-2.0, and never one outside `deny.toml`'s allow list. Apache-2.0 also
  carries a crate's `NOTICE` file.
- `canonical/`: standard texts of licenses without a per-work copyright line,
  used when a crate ships no file for them, and the MIT template.
- `crates.json` and `crates/`: clarifications for crates that ship no license
  file. Each one pins the upstream file at the crate's VCS revision by URL and
  SHA-256, and applies to a Cargo caret version range. A crate whose repository
  has no license file either uses `"standard": "authors"`: the MIT template with
  its Cargo.toml authors, or `holders`, as copyright holders. A version outside
  the range fails generation, and so does an unused clarification.
- `rust/`: the Rust standard library's notices at the pinned toolchain.
  Bumping `rust-toolchain.toml` requires refreshing them and their
  `provenance.json` records.
- `components.json`: everything that is not a crate. Files are repository
  paths, `crate:<name>/<path>` inside a crate's source, optionally narrowed to
  a header with `#L<first>-L<last>`, or `packaged:<file>` for a notice the
  Linux packager copies beside `licenses.json` at package time; generation
  checks each `packaged:` name against `assets/linux/package-policy.json`. A
  `$name` version reads the pinned version from its source manifest.

`provenance.json` here and in `third-party/ghostty/` record each fetched
file's origin and SHA-256; generation verifies them.

Entries record the targets that ship them. The app shows only the entries for
its own build: its target, the `macos-updater` feature for Sparkle, an
AppImage launch for the AppImage runtime, and packaged notices that exist.
