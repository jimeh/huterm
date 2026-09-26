use huterm_protocol::{
    CellText, LinkCell, LinkLookup, LinkSource, MousePosition, TerminalLink,
    TerminalSnapshot,
};

// Bound owner-thread work and every destination allocation independently.
pub(super) const MAX_LINK_BYTES: usize = 32 * 1024;
pub(super) const MAX_LINK_ROWS: usize = 128;
pub(super) const MAX_LINK_CELLS: usize = 16 * 1024;
const MAX_COMPARISON_BYTES: usize = 2 * 1024 * 1024;

pub(super) trait LinkBuffer {
    fn total_rows(&self) -> Result<usize, LinkLookup>;
    fn wrapped(&self, row: usize) -> Result<bool, LinkLookup>;
    /// Appends the cell's grapheme cluster, a space for an empty cell, or
    /// nothing for a wide-character spacer.
    fn push_cell(
        &mut self,
        row: usize,
        column: u16,
        text: &mut String,
    ) -> Result<(), LinkLookup>;
    fn hyperlink(
        &self,
        row: usize,
        column: u16,
    ) -> Result<Option<String>, LinkLookup>;
    fn same_hyperlink(
        &self,
        row: usize,
        column: u16,
        destination: &str,
        scratch: &mut [u8],
    ) -> Result<bool, LinkLookup>;
}

pub(super) fn resolve(
    buffer: &mut impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    point: MousePosition,
) -> LinkLookup {
    resolve_inner(buffer, snapshot, point).unwrap_or_else(|outcome| outcome)
}

fn resolve_inner(
    buffer: &mut impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    point: MousePosition,
) -> Result<LinkLookup, LinkLookup> {
    let rows = usize::from(snapshot.size.rows);
    if point.column >= u32::from(snapshot.size.columns)
        || point.row >= u32::from(snapshot.size.rows)
    {
        return Ok(LinkLookup::NoMatch);
    }
    let total = buffer.total_rows()?;
    let top = total
        .checked_sub(rows + snapshot.viewport.bottom_offset)
        .ok_or(LinkLookup::Unavailable)?;
    let row = top + point.row as usize;
    let column =
        u16::try_from(point.column).map_err(|_| LinkLookup::Unavailable)?;
    if let Some(destination) = buffer.hyperlink(row, column)? {
        return resolve_explicit(
            buffer,
            snapshot,
            destination,
            top,
            total,
            row,
            column,
        );
    }
    resolve_plain(buffer, snapshot, top, total, row, column)
}

/// Resolves a plain-text URL from the token under the pointer.
///
/// A token ends at the nearest delimiter on either side, so text beyond them
/// cannot change the result. The scan reads outward from the pointer and stops
/// at those delimiters rather than reading the whole soft-wrapped line; the
/// scan limits apply to this window.
fn resolve_plain(
    buffer: &mut impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    top: usize,
    total: usize,
    row: usize,
    column: u16,
) -> Result<LinkLookup, LinkLookup> {
    let columns = snapshot.size.columns;
    let mut window = Window {
        cells: 0,
        bytes: 0,
        first_line: row,
        last_line: row,
    };
    let mut after = String::new();
    let mut after_cells = Vec::new();
    let mut position = Some((row, column));
    while let Some((line, cell_column)) = position {
        let start = after.len();
        buffer.push_cell(line, cell_column, &mut after)?;
        after_cells.push((line, cell_column, start, after.len()));
        let cell = &after[start..];
        if (line, cell_column) == (row, column)
            && cell.starts_with(is_delimiter)
        {
            // The pointer is on a delimiter, which no link includes.
            return Ok(LinkLookup::NoMatch);
        }
        // A delimiter bounds the token without counting against its limits.
        if cell.contains(is_delimiter) {
            break;
        }
        window.admit(line, cell.len())?;
        position = next_cell(buffer, total, columns, line, cell_column)?;
    }
    // Cells before the target, in reverse reading order.
    let mut before = String::new();
    let mut before_cells = Vec::new();
    let mut position = previous_cell(buffer, columns, row, column)?;
    while let Some((line, cell_column)) = position {
        let start = before.len();
        buffer.push_cell(line, cell_column, &mut before)?;
        before_cells.push((line, cell_column, start, before.len()));
        if before[start..].contains(is_delimiter) {
            break;
        }
        window.admit(line, before.len() - start)?;
        position = previous_cell(buffer, columns, line, cell_column)?;
    }
    // Backward cells are in reverse order, so a cell at [start, end) in
    // `before` moves to [len - end, len - start) in reading order.
    let base = before.len();
    let mut mapping =
        Vec::with_capacity(before_cells.len() + after_cells.len());
    mapping.extend(before_cells.iter().rev().map(
        |&(line, cell_column, start, end)| {
            (line, cell_column, base - end, base - start)
        },
    ));
    let mut text = if before_cells.iter().all(|cell| cell.3 - cell.2 == 1) {
        // Reversing single-byte cells restores their reading order.
        let mut bytes = before.into_bytes();
        bytes.reverse();
        String::from_utf8(bytes).map_err(|_| LinkLookup::Unavailable)?
    } else {
        let mut text = String::with_capacity(base + after.len());
        for &(_, _, start, end) in before_cells.iter().rev() {
            text.push_str(&before[start..end]);
        }
        text
    };
    text.push_str(&after);
    // An empty target is a wide spacer, which belongs to the preceding
    // character.
    let target_offset = if after_cells[0].2 == after_cells[0].3 {
        base.saturating_sub(1)
    } else {
        base
    };
    mapping.extend(after_cells.into_iter().map(
        |(line, cell_column, start, end)| {
            (line, cell_column, base + start, base + end)
        },
    ));
    Ok(plain_link(
        &text,
        mapping,
        target_offset,
        top,
        usize::from(snapshot.size.rows),
    ))
}

/// Bounds the rows, cells, and bytes of the token a plain-text scan reads.
struct Window {
    cells: usize,
    bytes: usize,
    first_line: usize,
    last_line: usize,
}

impl Window {
    fn admit(&mut self, line: usize, bytes: usize) -> Result<(), LinkLookup> {
        self.cells += 1;
        self.bytes += bytes;
        self.first_line = self.first_line.min(line);
        self.last_line = self.last_line.max(line);
        if self.cells > MAX_LINK_CELLS
            || self.last_line - self.first_line >= MAX_LINK_ROWS
            || self.bytes > MAX_LINK_BYTES
        {
            return Err(LinkLookup::ScanLimit);
        }
        Ok(())
    }
}

fn next_cell(
    buffer: &impl LinkBuffer,
    total: usize,
    columns: u16,
    line: usize,
    column: u16,
) -> Result<Option<(usize, u16)>, LinkLookup> {
    if column + 1 < columns {
        return Ok(Some((line, column + 1)));
    }
    Ok((line + 1 < total && buffer.wrapped(line)?).then_some((line + 1, 0)))
}

fn previous_cell(
    buffer: &impl LinkBuffer,
    columns: u16,
    line: usize,
    column: u16,
) -> Result<Option<(usize, u16)>, LinkLookup> {
    if column > 0 {
        return Ok(Some((line, column - 1)));
    }
    Ok(
        (line > 0 && buffer.wrapped(line - 1)?)
            .then(|| (line - 1, columns - 1)),
    )
}

/// Matches the URL at `target_offset` in `text`, whose cells are described by
/// `(line, column, start byte, end byte)` in reading order.
fn plain_link(
    text: &str,
    mapping: Vec<(usize, u16, usize, usize)>,
    target_offset: usize,
    top: usize,
    rows: usize,
) -> LinkLookup {
    let Some((start, end)) = plain_range(text, target_offset) else {
        return LinkLookup::NoMatch;
    };
    let destination = text[start..end].to_owned();
    let cells = mapping
        .into_iter()
        .filter_map(|(line, column, start_byte, end_byte)| {
            // A wide spacer has no bytes but belongs to the preceding character.
            let belongs = if start_byte == end_byte {
                start_byte > start && start_byte <= end
            } else {
                start_byte >= start && end_byte <= end
            };
            (belongs && line >= top && line < top + rows).then(|| LinkCell {
                position: MousePosition {
                    row: u32::try_from(line - top).unwrap_or(u32::MAX),
                    column: u32::from(column),
                },
                text: CellText::new(&text[start_byte..end_byte]),
            })
        })
        .collect();
    LinkLookup::Match(TerminalLink {
        destination,
        source: LinkSource::PlainText,
        cells,
    })
}

fn resolve_explicit(
    buffer: &mut impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    destination: String,
    top: usize,
    total: usize,
    row: usize,
    column: u16,
) -> Result<LinkLookup, LinkLookup> {
    let columns = usize::from(snapshot.size.columns);
    let rows = usize::from(snapshot.size.rows);
    if !valid_destination(&destination) {
        return Ok(LinkLookup::NoMatch);
    }
    let index = row
        .checked_mul(columns)
        .and_then(|index| index.checked_add(usize::from(column)))
        .ok_or(LinkLookup::Unavailable)?;
    let mut scratch = vec![0; destination.len()];
    let mut first = index;
    let mut last = index;
    let mut count = 1;
    // Expand only contiguous cells with this exact explicit destination.
    while first > 0 {
        let previous = first - 1;
        if !buffer.same_hyperlink(
            previous / columns,
            u16::try_from(previous % columns)
                .map_err(|_| LinkLookup::Unavailable)?,
            &destination,
            &mut scratch,
        )? {
            break;
        }
        first = previous;
        count += 1;
        if count > MAX_LINK_CELLS
            || count.saturating_mul(destination.len()) > MAX_COMPARISON_BYTES
            || (last / columns - first / columns) >= MAX_LINK_ROWS
        {
            return Err(LinkLookup::ScanLimit);
        }
    }
    while last + 1 < total.saturating_mul(columns) {
        let next = last + 1;
        if !buffer.same_hyperlink(
            next / columns,
            u16::try_from(next % columns)
                .map_err(|_| LinkLookup::Unavailable)?,
            &destination,
            &mut scratch,
        )? {
            break;
        }
        last = next;
        count += 1;
        if count > MAX_LINK_CELLS
            || count.saturating_mul(destination.len()) > MAX_COMPARISON_BYTES
            || (last / columns - first / columns) >= MAX_LINK_ROWS
        {
            return Err(LinkLookup::ScanLimit);
        }
    }
    let mut cells = Vec::new();
    let mut text = String::new();
    for index in first.max(top * columns)..=last.min((top + rows) * columns - 1)
    {
        let start = text.len();
        buffer.push_cell(
            index / columns,
            u16::try_from(index % columns)
                .map_err(|_| LinkLookup::Unavailable)?,
            &mut text,
        )?;
        if text.len() > MAX_LINK_BYTES {
            return Err(LinkLookup::ScanLimit);
        }
        cells.push(LinkCell {
            position: MousePosition {
                row: u32::try_from(index / columns - top)
                    .map_err(|_| LinkLookup::Unavailable)?,
                column: u32::try_from(index % columns)
                    .map_err(|_| LinkLookup::Unavailable)?,
            },
            text: CellText::new(&text[start..]),
        });
    }
    Ok(LinkLookup::Match(TerminalLink {
        destination,
        source: LinkSource::Osc8,
        cells,
    }))
}

fn valid_destination(text: &str) -> bool {
    if text.len() > MAX_LINK_BYTES
        || text.chars().any(|c| {
            c.is_control()
                || c.is_whitespace()
                || matches!(c, '\\' | '<' | '>' | '"')
        })
    {
        return false;
    }
    // Parsing is validation only. Keep the exact destination rather than URL
    // normalization so activation cannot substitute another spelling.
    url::Url::parse(text).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
    })
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\\')
}

fn plain_range(text: &str, point: usize) -> Option<(usize, usize)> {
    // Find the one token under the cell. Each byte is inspected a bounded
    // number of times, including URL-heavy and unmatched-punctuation input.
    let mut token_start = 0;
    for (offset, character) in text.char_indices() {
        if is_delimiter(character) {
            if offset >= point {
                return token_range(
                    &text[token_start..offset],
                    token_start,
                    point,
                );
            }
            token_start = offset + character.len_utf8();
        }
    }
    token_range(&text[token_start..], token_start, point)
}

fn token_range(
    token: &str,
    token_start: usize,
    point: usize,
) -> Option<(usize, usize)> {
    let start = token
        .as_bytes()
        .windows(7)
        .position(|bytes| bytes.eq_ignore_ascii_case(b"http://"))
        .into_iter()
        .chain(
            token
                .as_bytes()
                .windows(8)
                .position(|bytes| bytes.eq_ignore_ascii_case(b"https://")),
        )
        .min()?;
    let candidate = &token[start..];
    let mut balances = [0i32; 3];
    for character in candidate.chars() {
        match character {
            '(' => balances[0] += 1,
            ')' => balances[0] -= 1,
            '[' => balances[1] += 1,
            ']' => balances[1] -= 1,
            '{' => balances[2] += 1,
            '}' => balances[2] -= 1,
            _ => {}
        }
    }
    let mut end = candidate.len();
    for character in candidate.chars().rev() {
        let bracket = match character {
            ')' => Some(0),
            ']' => Some(1),
            '}' => Some(2),
            _ => None,
        };
        if let Some(index) = bracket.filter(|index| balances[*index] < 0) {
            balances[index] += 1;
        } else if !matches!(character, '.' | ',' | ';' | ':' | '!' | '?') {
            break;
        }
        end -= character.len_utf8();
    }
    let start = token_start + start;
    (point >= start
        && point < start + end
        && valid_destination(&candidate[..end]))
    .then_some((start, start + end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{GridSize, TerminalId, TerminalModes, Viewport};

    /// Rows of cell text, where "" is a wide spacer, and their wrap flags.
    struct Grid {
        columns: u16,
        rows: Vec<(Vec<&'static str>, bool)>,
    }

    impl Grid {
        fn cell(&self, row: usize, column: u16) -> Result<&str, LinkLookup> {
            self.rows
                .get(row)
                .and_then(|(cells, _)| cells.get(usize::from(column)))
                .copied()
                .ok_or(LinkLookup::Unavailable)
        }
    }

    impl LinkBuffer for Grid {
        fn total_rows(&self) -> Result<usize, LinkLookup> {
            Ok(self.rows.len())
        }
        fn wrapped(&self, row: usize) -> Result<bool, LinkLookup> {
            self.rows
                .get(row)
                .map(|(_, wrapped)| *wrapped)
                .ok_or(LinkLookup::Unavailable)
        }
        fn push_cell(
            &mut self,
            row: usize,
            column: u16,
            text: &mut String,
        ) -> Result<(), LinkLookup> {
            text.push_str(self.cell(row, column)?);
            Ok(())
        }
        fn hyperlink(
            &self,
            _: usize,
            _: u16,
        ) -> Result<Option<String>, LinkLookup> {
            Ok(None)
        }
        fn same_hyperlink(
            &self,
            _: usize,
            _: u16,
            _: &str,
            _: &mut [u8],
        ) -> Result<bool, LinkLookup> {
            Ok(false)
        }
    }

    /// The previous whole-line scan, without limits.
    fn whole_line(
        grid: &Grid,
        top: usize,
        rows: usize,
        row: usize,
        column: u16,
    ) -> LinkLookup {
        let (mut first, mut last) = (row, row);
        while first > 0 && grid.rows[first - 1].1 {
            first -= 1;
        }
        while last + 1 < grid.rows.len() && grid.rows[last].1 {
            last += 1;
        }
        let mut text = String::new();
        let mut mapping = Vec::new();
        let mut target_offset = 0;
        for line in first..=last {
            for cell_column in 0..grid.columns {
                let offset = text.len();
                text.push_str(grid.cell(line, cell_column).unwrap());
                if line == row && cell_column == column {
                    target_offset = if text.len() == offset {
                        offset.saturating_sub(1)
                    } else {
                        offset
                    };
                }
                mapping.push((line, cell_column, offset, text.len()));
            }
        }
        plain_link(&text, mapping, target_offset, top, rows)
    }

    // Includes a wide delimiter and a multi-codepoint grapheme.
    const TOKENS: [&str; 17] = [
        "https://", "http://", "HTTPS://", "a.test", "/p", "(", ")", "[", "]",
        " ", ".", ",", "界", "e\u{301}", "x", "'", "\u{3000}",
    ];
    const COLUMNS: u16 = 6;
    const ROWS: u16 = 4;

    /// Cells of one logical line, laid out as Ghostty wraps wide characters.
    fn random_line(
        random: &mut impl FnMut(usize) -> usize,
    ) -> Vec<&'static str> {
        let columns = usize::from(COLUMNS);
        let mut cells = Vec::new();
        for _ in 0..random(14) {
            let token = TOKENS[random(TOKENS.len())];
            for (index, character) in token.char_indices() {
                if character == '\u{301}' {
                    continue;
                }
                let end = token[index..]
                    .char_indices()
                    .skip(1)
                    .find(|(_, next)| *next != '\u{301}')
                    .map_or(token.len(), |(next, _)| index + next);
                let wide = matches!(character, '界' | '\u{3000}');
                if wide && cells.len() % columns == columns - 1 {
                    cells.push("");
                }
                cells.push(&token[index..end]);
                if wide {
                    cells.push("");
                }
            }
        }
        cells
    }

    fn random_grid(random: &mut impl FnMut(usize) -> usize) -> Grid {
        let columns = usize::from(COLUMNS);
        let mut grid = Grid {
            columns: COLUMNS,
            rows: Vec::new(),
        };
        for _ in 0..=random(4) {
            let cells = random_line(random);
            let chunks: Vec<_> = cells.chunks(columns).collect();
            for (index, chunk) in chunks.iter().enumerate() {
                let mut row = chunk.to_vec();
                row.resize(columns, " ");
                grid.rows.push((row, index + 1 < chunks.len()));
            }
            if chunks.is_empty() {
                grid.rows.push((vec![" "; columns], false));
            }
        }
        while grid.rows.len() < usize::from(ROWS) {
            grid.rows.push((vec![" "; columns], false));
        }
        grid
    }

    #[test]
    fn token_window_matches_whole_line_scan() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap() % bound
        };
        let mut matches = 0;
        for _ in 0..300 {
            let mut grid = random_grid(&mut random);
            let history = grid.rows.len() - usize::from(ROWS);
            let bottom_offset = random(history + 1);
            let top = history - bottom_offset;
            let snapshot = TerminalSnapshot {
                terminal_id: TerminalId::new(1),
                generation: 0,
                size: GridSize {
                    columns: COLUMNS,
                    rows: ROWS,
                },
                rows: Vec::new(),
                cursor: None,
                modes: TerminalModes::default(),
                viewport: Viewport { bottom_offset },
                history_size: history,
                cursor_color: None,
            };
            for row in 0..ROWS {
                for column in 0..COLUMNS {
                    let expected = whole_line(
                        &grid,
                        top,
                        usize::from(ROWS),
                        top + usize::from(row),
                        column,
                    );
                    let point = MousePosition {
                        row: u32::from(row),
                        column: u32::from(column),
                    };
                    assert_eq!(
                        resolve(&mut grid, &snapshot, point),
                        expected,
                        "{point:?} in {:?}",
                        grid.rows
                    );
                    matches +=
                        usize::from(matches!(expected, LinkLookup::Match(_)));
                }
            }
        }
        // Guard against a generator that never produces a link.
        assert!(matches > 100, "only {matches} matches compared");
    }

    #[test]
    fn matches_balanced_url_punctuation_and_rejects_unsupported_destinations() {
        let value = "(https://example.test/a_(b)?x=1&y=λ).";
        let (start, end) = plain_range(value, 15).unwrap();
        assert_eq!(&value[start..end], "https://example.test/a_(b)?x=1&y=λ");
        for value in [
            "file:///tmp/a",
            "javascript:alert(1)",
            "https://",
            "https://bad host",
            "https://a/\n",
            "https://a/\\b",
        ] {
            assert!(!valid_destination(value), "{value}");
        }
    }
}
