use huterm_protocol::{
    LinkCell, LinkLookup, LinkSource, MousePosition, TerminalLink,
    TerminalSnapshot,
};

// Bound owner-thread work and every destination allocation independently.
pub(super) const MAX_LINK_BYTES: usize = 32 * 1024;
pub(super) const MAX_LINK_ROWS: usize = 128;
pub(super) const MAX_LINK_CELLS: usize = 16 * 1024;
const MAX_COMPARISON_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct TextCell {
    pub text: String,
}

pub(super) trait LinkBuffer {
    fn total_rows(&self) -> Result<usize, LinkLookup>;
    fn wrapped(&self, row: usize) -> Result<bool, LinkLookup>;
    fn cell(&self, row: usize, column: u16) -> Result<TextCell, LinkLookup>;
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
    buffer: &impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    point: MousePosition,
) -> LinkLookup {
    resolve_inner(buffer, snapshot, point).unwrap_or_else(|outcome| outcome)
}

fn resolve_inner(
    buffer: &impl LinkBuffer,
    snapshot: &TerminalSnapshot,
    point: MousePosition,
) -> Result<LinkLookup, LinkLookup> {
    let columns = usize::from(snapshot.size.columns);
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
    let mut first = row;
    let mut last = row;
    while first > 0 && buffer.wrapped(first - 1)? {
        first -= 1;
        if row - first >= MAX_LINK_ROWS {
            return Err(LinkLookup::ScanLimit);
        }
    }
    while last + 1 < total && buffer.wrapped(last)? {
        last += 1;
        if last - first >= MAX_LINK_ROWS {
            return Err(LinkLookup::ScanLimit);
        }
    }
    if (last - first + 1).saturating_mul(columns) > MAX_LINK_CELLS {
        return Err(LinkLookup::ScanLimit);
    }
    let mut text = String::new();
    let mut mapping = Vec::new();
    let mut target_offset = None;
    for line in first..=last {
        for column in 0..snapshot.size.columns {
            let cell = buffer.cell(line, column)?;
            if text.len().saturating_add(cell.text.len()) > MAX_LINK_BYTES {
                return Err(LinkLookup::ScanLimit);
            }
            let offset = text.len();
            if line == row && u32::from(column) == point.column {
                target_offset = Some(if cell.text.is_empty() {
                    offset.saturating_sub(1)
                } else {
                    offset
                });
            }
            text.push_str(&cell.text);
            mapping.push((line, column, offset, text.len(), cell.text));
        }
    }
    let target_offset = target_offset.ok_or(LinkLookup::Unavailable)?;
    let Some((start, end)) = plain_range(&text, target_offset) else {
        return Ok(LinkLookup::NoMatch);
    };
    let destination = text[start..end].to_owned();
    let cells = mapping
        .into_iter()
        .filter_map(|(line, column, start_byte, end_byte, text)| {
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
                text,
            })
        })
        .collect();
    Ok(LinkLookup::Match(TerminalLink {
        destination,
        source: LinkSource::PlainText,
        cells,
    }))
}

fn resolve_explicit(
    buffer: &impl LinkBuffer,
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
    let mut bytes = 0;
    for index in first.max(top * columns)..=last.min((top + rows) * columns - 1)
    {
        let cell = buffer.cell(
            index / columns,
            u16::try_from(index % columns)
                .map_err(|_| LinkLookup::Unavailable)?,
        )?;
        bytes += cell.text.len();
        if bytes > MAX_LINK_BYTES {
            return Err(LinkLookup::ScanLimit);
        }
        cells.push(LinkCell {
            position: MousePosition {
                row: u32::try_from(index / columns - top)
                    .map_err(|_| LinkLookup::Unavailable)?,
                column: u32::try_from(index % columns)
                    .map_err(|_| LinkLookup::Unavailable)?,
            },
            text: cell.text,
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

fn plain_range(text: &str, point: usize) -> Option<(usize, usize)> {
    let delimiter = |c: char| {
        c.is_whitespace()
            || c.is_control()
            || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\\')
    };
    // Find the one token under the cell. Each byte is inspected a bounded
    // number of times, including URL-heavy and unmatched-punctuation input.
    let mut token_start = 0;
    for (offset, character) in text.char_indices() {
        if delimiter(character) {
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
