use std::cmp::Ordering;
use std::collections::HashMap;

use huterm_protocol::{CommandId, CommandSpec};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const RECENT_COMMAND_LIMIT: usize = 32;

/// Which catalog field produced a match. Lower is better.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum MatchField {
    Title,
    Id,
    Description,
}

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

/// Sources of usage history the ranker consults. In-memory now; a persisted
/// store can implement this later.
pub(crate) trait CommandHistory {
    /// Position in this window's recent list, 0 = most recent.
    fn recency(&self, id: CommandId) -> Option<usize>;

    /// Process-wide use count.
    fn frequency(&self, id: CommandId) -> u32;
}

pub(crate) struct CommandSearch {
    matcher: Matcher,
    haystack: Vec<char>,
    indices: Vec<u32>,
}

impl CommandSearch {
    pub(crate) fn new() -> Self {
        Self {
            matcher: Matcher::new(matcher_config()),
            haystack: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Ranks `candidates` for `query`.
    ///
    /// An empty query returns every candidate, ordered by recency, frequency,
    /// title, then catalog position. A non-empty query ranks field priority
    /// before matcher score, then uses recency, frequency, and catalog order
    /// as tie-breakers.
    pub(crate) fn rank(
        &mut self,
        query: &str,
        candidates: impl IntoIterator<Item = &'static CommandSpec>,
        history: &dyn CommandHistory,
    ) -> Vec<CommandMatch> {
        if query.is_empty() {
            return rank_empty(candidates, history);
        }

        let pattern = fuzzy_pattern(query);
        let mut matches = candidates
            .into_iter()
            .enumerate()
            .filter_map(|(catalog_order, spec)| {
                self.match_command(&pattern, spec)
                    .map(|matched| RankedMatch {
                        recency: history.recency(spec.id).unwrap_or(usize::MAX),
                        frequency: history.frequency(spec.id),
                        catalog_order,
                        matched,
                    })
            })
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| {
            let field_order = left.matched.field.cmp(&right.matched.field);
            if field_order != Ordering::Equal {
                return field_order;
            }

            right
                .matched
                .score
                .cmp(&left.matched.score)
                .then_with(|| left.recency.cmp(&right.recency))
                .then_with(|| right.frequency.cmp(&left.frequency))
                .then_with(|| left.catalog_order.cmp(&right.catalog_order))
        });
        matches.into_iter().map(|ranked| ranked.matched).collect()
    }

    fn match_command(
        &mut self,
        pattern: &Pattern,
        spec: &'static CommandSpec,
    ) -> Option<CommandMatch> {
        if let Some((score, title_indices)) =
            self.match_title(pattern, spec.title)
        {
            return Some(CommandMatch {
                spec,
                field: MatchField::Title,
                score,
                title_indices,
            });
        }
        if let Some(score) = self.score(pattern, spec.id.as_str()) {
            return Some(CommandMatch {
                spec,
                field: MatchField::Id,
                score,
                title_indices: Vec::new(),
            });
        }
        self.score(pattern, spec.description)
            .map(|score| CommandMatch {
                spec,
                field: MatchField::Description,
                score,
                title_indices: Vec::new(),
            })
    }

    fn match_title(
        &mut self,
        pattern: &Pattern,
        title: &str,
    ) -> Option<(u32, Vec<u32>)> {
        self.haystack.clear();
        self.indices.clear();
        let haystack = Utf32Str::new(title, &mut self.haystack);
        let score =
            pattern.indices(haystack, &mut self.matcher, &mut self.indices)?;
        let byte_offsets = title
            .char_indices()
            .filter_map(|(offset, _)| u32::try_from(offset).ok())
            .collect::<Vec<_>>();
        let title_indices = self
            .indices
            .iter()
            .filter_map(|index| byte_offsets.get(*index as usize).copied())
            .collect();
        Some((score, title_indices))
    }

    fn score(&mut self, pattern: &Pattern, text: &str) -> Option<u32> {
        self.haystack.clear();
        let haystack = Utf32Str::new(text, &mut self.haystack);
        pattern.score(haystack, &mut self.matcher)
    }
}

struct RankedMatch {
    matched: CommandMatch,
    recency: usize,
    frequency: u32,
    catalog_order: usize,
}

fn rank_empty(
    candidates: impl IntoIterator<Item = &'static CommandSpec>,
    history: &dyn CommandHistory,
) -> Vec<CommandMatch> {
    let mut matches = candidates
        .into_iter()
        .enumerate()
        .map(|(catalog_order, spec)| RankedMatch {
            matched: CommandMatch {
                spec,
                field: MatchField::Title,
                score: 0,
                title_indices: Vec::new(),
            },
            recency: history.recency(spec.id).unwrap_or(usize::MAX),
            frequency: history.frequency(spec.id),
            catalog_order,
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.recency
            .cmp(&right.recency)
            .then_with(|| right.frequency.cmp(&left.frequency))
            .then_with(|| left.matched.spec.title.cmp(right.matched.spec.title))
            .then_with(|| left.catalog_order.cmp(&right.catalog_order))
    });
    matches.into_iter().map(|ranked| ranked.matched).collect()
}

/// Fuzzy filter for picker rows, using the command matcher's configuration.
pub(crate) struct PickerMatcher {
    matcher: Matcher,
    haystack: Vec<char>,
}

impl PickerMatcher {
    pub(crate) fn new() -> Self {
        Self {
            matcher: Matcher::new(matcher_config()),
            haystack: Vec::new(),
        }
    }

    /// Returns matching row indices, best first.
    ///
    /// An empty query preserves the original order. Otherwise label matches
    /// rank above detail-only matches.
    pub(crate) fn filter(
        &mut self,
        query: &str,
        rows: impl Iterator<Item = (String, String)>,
    ) -> Vec<usize> {
        if query.is_empty() {
            return rows.enumerate().map(|(index, _)| index).collect();
        }

        let pattern = fuzzy_pattern(query);
        let mut matches = rows
            .enumerate()
            .filter_map(|(index, (label, detail))| {
                self.score(&pattern, &label)
                    .map(|score| (MatchField::Title, score, index))
                    .or_else(|| {
                        self.score(&pattern, &detail).map(|score| {
                            (MatchField::Description, score, index)
                        })
                    })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        matches.into_iter().map(|(_, _, index)| index).collect()
    }

    fn score(&mut self, pattern: &Pattern, text: &str) -> Option<u32> {
        self.haystack.clear();
        let haystack = Utf32Str::new(text, &mut self.haystack);
        pattern.score(haystack, &mut self.matcher)
    }
}

/// In-memory per-window command recency, most recent first.
#[derive(Debug, Default)]
pub(crate) struct RecentCommands {
    ids: Vec<CommandId>,
}

impl RecentCommands {
    pub(crate) fn record(&mut self, id: CommandId) {
        self.ids.retain(|recorded| *recorded != id);
        self.ids.insert(0, id);
        self.ids.truncate(RECENT_COMMAND_LIMIT);
    }
}

/// In-memory process-wide command use counts.
#[derive(Debug, Default)]
pub(crate) struct CommandFrequency {
    counts: HashMap<CommandId, u32>,
}

impl CommandFrequency {
    pub(crate) fn record(&mut self, id: CommandId) {
        let count = self.counts.entry(id).or_default();
        *count = count.saturating_add(1);
    }
}

/// Combines a window's recency with process-wide frequency for ranking.
pub(crate) struct HistoryView<'a> {
    pub(crate) recent: &'a RecentCommands,
    pub(crate) frequency: &'a CommandFrequency,
}

impl CommandHistory for HistoryView<'_> {
    fn recency(&self, id: CommandId) -> Option<usize> {
        self.recent.ids.iter().position(|recorded| *recorded == id)
    }

    fn frequency(&self, id: CommandId) -> u32 {
        self.frequency.counts.get(&id).copied().unwrap_or(0)
    }
}

fn matcher_config() -> Config {
    let mut config = Config::DEFAULT;
    config.ignore_case = true;
    config.normalize = true;
    config
}

fn fuzzy_pattern(query: &str) -> Pattern {
    let mut pattern =
        Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    for atom in &mut pattern.atoms {
        atom.kind = AtomKind::Fuzzy;
    }
    pattern
}

#[cfg(test)]
mod tests {
    use huterm_protocol::{catalog, ids};

    use super::*;

    fn history_view<'a>(
        recent: &'a RecentCommands,
        frequency: &'a CommandFrequency,
    ) -> HistoryView<'a> {
        HistoryView { recent, frequency }
    }

    fn specs(ids: &[CommandId]) -> Vec<&'static CommandSpec> {
        ids.iter()
            .filter_map(|id| catalog().iter().find(|spec| spec.id == *id))
            .collect()
    }

    #[test]
    fn fuzzy_subsequence_finds_title() {
        let recent = RecentCommands::default();
        let frequency = CommandFrequency::default();
        let mut search = CommandSearch::new();

        let matches =
            search.rank("tfs", catalog(), &history_view(&recent, &frequency));

        assert_eq!(matches[0].spec.id, ids::TOGGLE_FULLSCREEN);
        assert_eq!(matches[0].title_indices, [0, 7, 11]);
        assert!(matches.iter().any(|matched| {
            matched.spec.id == ids::TOGGLE_NATIVE_FULLSCREEN
        }));
        assert!(matches.iter().any(|matched| {
            matched.spec.id == ids::TOGGLE_NON_NATIVE_FULLSCREEN
        }));
    }

    #[test]
    fn field_priority_beats_score() {
        let recent = RecentCommands::default();
        let frequency = CommandFrequency::default();
        let mut search = CommandSearch::new();
        let candidates = specs(&[ids::SELECT_TAB, ids::RENAME_TAB]);

        // "set tab" weakly matches the Select Tab title, but matches "Set a
        // custom name for a tab" more strongly in Rename Tab's description.
        let matches = search.rank(
            "set tab",
            candidates,
            &history_view(&recent, &frequency),
        );

        assert_eq!(matches[0].spec.id, ids::SELECT_TAB);
        assert_eq!(matches[0].field, MatchField::Title);
        assert_eq!(matches[1].spec.id, ids::RENAME_TAB);
        assert_eq!(matches[1].field, MatchField::Description);
        assert!(matches[1].score > matches[0].score);
    }

    #[test]
    fn id_matches_rank_below_titles_and_above_descriptions() {
        let recent = RecentCommands::default();
        let frequency = CommandFrequency::default();
        let mut search = CommandSearch::new();
        let candidates = specs(&[
            ids::SCROLL_TO_BOTTOM,
            ids::SELECT_RECENT_TAB,
            ids::OPEN_COMMAND_PALETTE,
        ]);

        let matches =
            search.rank("srt", candidates, &history_view(&recent, &frequency));

        assert_eq!(
            matches
                .iter()
                .map(|matched| (matched.spec.id, matched.field))
                .collect::<Vec<_>>(),
            [
                (ids::SCROLL_TO_BOTTOM, MatchField::Title),
                (ids::SELECT_RECENT_TAB, MatchField::Id),
                (ids::OPEN_COMMAND_PALETTE, MatchField::Description),
            ]
        );
    }

    #[test]
    fn empty_query_orders_recent_then_frequent_then_alphabetical() {
        let mut recent = RecentCommands::default();
        recent.record(ids::QUIT);
        recent.record(ids::RENAME_TAB);
        let mut frequency = CommandFrequency::default();
        for _ in 0..5 {
            frequency.record(ids::NEW_TAB);
        }
        let mut search = CommandSearch::new();

        let matches =
            search.rank("", catalog(), &history_view(&recent, &frequency));

        assert_eq!(
            matches
                .iter()
                .take(4)
                .map(|matched| matched.spec.id)
                .collect::<Vec<_>>(),
            [ids::RENAME_TAB, ids::QUIT, ids::NEW_TAB, ids::ABOUT]
        );
    }

    #[test]
    fn recency_breaks_ties_within_a_field() {
        let mut recent = RecentCommands::default();
        recent.record(ids::RENAME_WORKSPACE);
        let frequency = CommandFrequency::default();
        let mut search = CommandSearch::new();
        let candidates = specs(&[ids::RENAME_TAB, ids::RENAME_WORKSPACE]);

        let matches = search.rank(
            "rename",
            candidates,
            &history_view(&recent, &frequency),
        );

        assert_eq!(matches[0].spec.id, ids::RENAME_WORKSPACE);
        assert_eq!(matches[0].score, matches[1].score);
    }

    #[test]
    fn history_records_and_caps() {
        let mut recent = RecentCommands::default();
        for spec in catalog().iter().take(33) {
            recent.record(spec.id);
        }
        let repeated = catalog()[1].id;
        recent.record(repeated);
        let mut frequency = CommandFrequency::default();
        frequency.record(repeated);
        frequency.record(repeated);
        let view = history_view(&recent, &frequency);

        assert_eq!(recent.ids.len(), RECENT_COMMAND_LIMIT);
        assert_eq!(view.recency(repeated), Some(0));
        assert_eq!(view.frequency(repeated), 2);
        assert_eq!(view.recency(catalog()[0].id), None);
    }

    #[test]
    fn picker_filter_prefers_labels_over_details() {
        let rows = || {
            [
                ("ssh build-01".to_owned(), "3 · Session 1".to_owned()),
                ("zsh".to_owned(), "1 · current".to_owned()),
            ]
            .into_iter()
        };
        let mut matcher = PickerMatcher::new();

        assert_eq!(matcher.filter("bld", rows()), [0]);
        assert_eq!(matcher.filter("current", rows()), [1]);
        assert_eq!(matcher.filter("", rows()), [0, 1]);
    }

    #[test]
    fn no_match_returns_empty() {
        let recent = RecentCommands::default();
        let frequency = CommandFrequency::default();
        let mut search = CommandSearch::new();

        let matches = search.rank(
            "zzzzzzzzzzzzzz",
            catalog(),
            &history_view(&recent, &frequency),
        );

        assert!(matches.is_empty());
    }
}
