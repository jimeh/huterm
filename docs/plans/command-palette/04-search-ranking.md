# Chunk 4: search and ranking engine

Part of [the command palette plan](../command-palette.md). Implements the
"Search and ranking" section as a pure module with no UI. Requires
[chunk 1](01-protocol-catalog.md) merged.

Owner: Codex implementer. Dependencies: chunk 1. Runs in parallel with
chunks 2 and 3. New files only, plus the dependency declaration.

## Objective

A `desktop/palette/search.rs` module (create the `palette` directory module
if `palette.rs` is still a single file: move `palette.rs` to
`palette/mod.rs` unchanged and add `search.rs` beside it) that ranks catalog
commands for a query using `nucleo-matcher`, per-window recency, and
process-wide frequency, and a `PickerMatcher` the pickers reuse.

Files: `Cargo.toml` (workspace dependencies), `crates/huterm-gpui/Cargo.toml`,
`Cargo.lock`, `crates/huterm-gpui/src/desktop/palette/search.rs`, and the
module move. Do not edit `palette/mod.rs` beyond `mod search;` and a
`pub(super) use` line, and do not touch `windows.rs`.

## Dependency

Add `nucleo-matcher = "=0.3.1"` to `[workspace.dependencies]` with the
existing exact-pin style, and `nucleo-matcher.workspace = true` under the
`huterm-gpui` platform dependencies. It is MPL-2.0, which `deny.toml`
allows. Run `mise run license` and confirm it passes; the release is well
outside the three-day cooldown. Do not add any other crate.

## Interfaces

```rust
/// Which catalog field produced a match. Lower is better.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum MatchField { Title, Id, Description }

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CommandMatch {
    pub(crate) spec: &'static CommandSpec,
    pub(crate) field: MatchField,
    /// Matcher score; only comparable within the same field.
    pub(crate) score: u32,
    /// Byte indices into `spec.title` to highlight; empty unless
    /// `field == Title`.
    pub(crate) title_indices: Vec<u32>,
}

/// Sources of usage history the ranker consults. In-memory now; a
/// persisted store can implement this later.
pub(crate) trait CommandHistory {
    /// Position in this window's recent list, 0 = most recent.
    fn recency(&self, id: CommandId) -> Option<usize>;
    /// Process-wide use count.
    fn frequency(&self, id: CommandId) -> u32;
}

pub(crate) struct CommandSearch {
    matcher: nucleo_matcher::Matcher,
    // reusable buffers
}

impl CommandSearch {
    pub(crate) fn new() -> Self;

    /// Ranks `candidates` for `query`. Empty query returns every candidate:
    /// recent first (by recency position), then by frequency descending,
    /// then alphabetical by title. Non-empty query: field priority, then
    /// score descending, then recency, then frequency, then catalog order.
    pub(crate) fn rank<'a>(
        &mut self,
        query: &str,
        candidates: impl IntoIterator<Item = &'static CommandSpec>,
        history: &dyn CommandHistory,
    ) -> Vec<CommandMatch>;
}

/// Fuzzy filter for picker rows, sharing the matcher configuration.
pub(crate) struct PickerMatcher { /* Matcher */ }

impl PickerMatcher {
    pub(crate) fn new() -> Self;
    /// Returns indices into `labels` that match, best first; every index
    /// in original order when `query` is empty. Matches against
    /// `label` first and `detail` second, label matches ranked above
    /// detail-only matches.
    pub(crate) fn filter(
        &mut self,
        query: &str,
        rows: impl Iterator<Item = (String, String)>,
    ) -> Vec<usize>;
}

/// In-memory history: per-window recency, most recent first, capped at 32.
#[derive(Debug, Default)]
pub(crate) struct RecentCommands { ids: Vec<CommandId> }
impl RecentCommands { pub(crate) fn record(&mut self, id: CommandId); }

#[derive(Debug, Default)]
pub(crate) struct CommandFrequency { counts: HashMap<CommandId, u32> }
impl CommandFrequency { pub(crate) fn record(&mut self, id: CommandId); }

/// Combines a window's recency with the process frequency for ranking.
pub(crate) struct HistoryView<'a> {
    pub(crate) recent: &'a RecentCommands,
    pub(crate) frequency: &'a CommandFrequency,
}
impl CommandHistory for HistoryView<'_> { ... }
```

Matcher configuration: `nucleo_matcher::Config::DEFAULT` with
`ignore_case = true` and `normalize = true`; use `Pattern::parse` with
`CaseMatching::Ignore` and `Normalization::Smart`, atom kind `Fuzzy`.
Match the title, then the ID with underscores treated as word separators
(nucleo handles `_` as a boundary already; verify with a test), then the
description. A command appears once, under its best field. Use
`Utf32Str` buffers reused across calls to avoid per-row allocation; the
catalog is small, so this is about tidiness, not speed.

Score comparability: nucleo scores depend on haystack length, so the plan's
"lexicographic field priority, then score" is exactly what `rank`
implements. Never compare scores across fields.

## Tests

In `search.rs`, named:

- `fuzzy_subsequence_finds_title`: `tfs` ranks `toggle_fullscreen` first,
  with `title_indices` covering `T`, `F`, `s` of "Toggle Fullscreen";
  `toggle_native_fullscreen` and `toggle_non_native_fullscreen` also match.
- `field_priority_beats_score`: construct a query where a description-only
  match would score higher than a weak title match, and assert the title
  match ranks first. Use catalog entries (for example `reload` matches
  "Reload Configuration" in the title and appears in no other title; pick
  a query that hits a long description strongly and a title weakly, and
  document the pair in the test).
- `id_matches_rank_below_titles_and_above_descriptions`: `reload_config`
  matched by `rlc` through the ID ranks below any title match for the same
  query and above description-only matches.
- `empty_query_orders_recent_then_frequent_then_alphabetical`: history
  with recency `[rename_tab, quit]` and frequency `{new_tab: 5}` yields
  `rename_tab, quit, new_tab, about, ...` (alphabetical after).
- `recency_breaks_ties_within_a_field`: two commands with identical field
  and score (construct with equal titles under a query matching a shared
  prefix, or use `rename_tab` and `rename_workspace` with query `rename`),
  the recent one first.
- `history_records_and_caps`: `RecentCommands` moves a repeated id to the
  front and caps at 32; `CommandFrequency` counts.
- `picker_filter_prefers_labels_over_details`: rows
  `("ssh build-01", "3 · Session 1")`, `("zsh", "1 · current")`; query
  `bld` returns the first; query `current` returns the second; empty query
  returns both in order.
- `no_match_returns_empty`: a query with characters absent from every
  field yields an empty vector.

Use a targeted perturbation on `field_priority_beats_score`: temporarily
sort by raw score and confirm the test fails at its assertion, then restore.
Record in the report that this was done.

## Verification

```sh
mise run license
cargo test --locked -p huterm-gpui palette::search
cargo clippy --locked -p huterm-gpui --all-targets -- -D warnings
cargo fmt --check
mise run architecture
```

## Self-review checklist

- Only `nucleo-matcher` was added; `Cargo.lock` diff contains it and its
  transitive dependencies only.
- `rank` never compares scores across fields.
- No UI, GPUI entity, or `windows.rs` code was touched.
- The module move left `palette/mod.rs` byte-identical to the old
  `palette.rs` apart from the two added lines.

Report: files changed, `mise run license` output tail, test names and
results, the perturbation evidence, and deviations with reasons.
