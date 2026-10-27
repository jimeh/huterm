//! Source-preserving edits to one TOML text.
//!
//! [`Layout`] indexes a parsed file as whole-line items: table headers and
//! key/value statements, each owning the comment block directly above it.
//! Every operation here becomes a list of byte-range [`Edits`] against that
//! text, so every byte outside the edited ranges is copied unchanged. After
//! each operation [`verify`] reparses the result and refuses it unless its
//! value tree is exactly the expected one and every comment outside the
//! removed ranges survived.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use toml::{Table, Value};

use super::{ApplyError, List};

/// One step of a path into the file's value tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Seg {
    Key(String),
    Index(usize),
}

impl Seg {
    fn key(&self) -> Option<&str> {
        match self {
            Self::Key(key) => Some(key),
            Self::Index(_) => None,
        }
    }
}

/// Tables of `RawConfig` where an empty one means the same as an absent
/// one: sections with fixed fields, and the quake profile map. One may be
/// removed once it has no keys and no comments.
const GROUPING_TABLES: &[&[&str]] = &[
    &["general"],
    &["terminal"],
    &["terminal", "bell"],
    &["font"],
    &["window"],
    &["tabs"],
    &["updates"],
    &["palette"],
    &["panes"],
    &["theme"],
    &["quake"],
    &["quake", "profiles"],
];

fn is_grouping(path: &[Seg]) -> bool {
    GROUPING_TABLES.iter().any(|grouping| {
        grouping.len() == path.len()
            && grouping
                .iter()
                .zip(path)
                .all(|(name, seg)| seg.key() == Some(*name))
    })
}

/// Field order for rendered keybinding entries.
const ENTRY_FIELDS: &[&str] =
    &["key", "command", "when", "args", "description", "disabled"];

/// One primitive change, applied and verified on its own.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Op {
    /// Replace or add a value. Tables are written inline.
    Set(Vec<Seg>, Value),
    /// Remove a value; absent values are left alone.
    Remove(Vec<Seg>),
    AppendEntry(List, Table),
    RemoveEntry(List, usize),
}

#[derive(Debug)]
enum Kind {
    Header {
        array: bool,
    },
    Statement {
        value: Range<usize>,
        /// Index into `Layout::items` of the header this statement follows.
        section: Option<usize>,
    },
}

#[derive(Debug)]
struct Item {
    path: Vec<Seg>,
    kind: Kind,
    /// Whole lines, through the last line's ending.
    lines: Range<usize>,
    /// Where the item's leading comment block starts; `lines.start` without
    /// one.
    lead: usize,
}

impl Item {
    fn is_header(&self) -> bool {
        matches!(self.kind, Kind::Header { .. })
    }
}

/// The structure of one parsed text.
pub(super) struct Layout<'a> {
    source: &'a str,
    items: Vec<Item>,
    comments: Vec<Range<usize>>,
    pub(super) tree: Table,
    line_ending: &'static str,
}

impl<'a> Layout<'a> {
    pub(super) fn parse(source: &'a str) -> Result<Self, ApplyError> {
        let invalid = |error: &dyn std::fmt::Display| {
            ApplyError::InvalidFile(error.to_string())
        };
        let tree: Table =
            toml::from_str(source).map_err(|error| invalid(&error))?;
        let document = toml_edit::Document::parse(source)
            .map_err(|error| invalid(&error))?;
        let mut raw = Vec::new();
        walk(document.as_table(), &mut Vec::new(), &mut raw)?;
        raw.sort_by_key(|(start, ..)| *start);
        let mut items: Vec<Item> = Vec::with_capacity(raw.len());
        let mut section = None;
        for (start, end, path, kind) in raw {
            let lines = line_start(source, start)..line_end(source, end);
            let previous_end = items.last().map_or(0, |item| item.lines.end);
            let lead = leading_block(source, lines.start, previous_end);
            let kind = match kind {
                RawKind::Header { array } => {
                    section = Some(items.len());
                    Kind::Header { array }
                }
                RawKind::Statement { value } => {
                    Kind::Statement { value, section }
                }
            };
            items.push(Item {
                path,
                kind,
                lines,
                lead,
            });
        }
        Ok(Self {
            source,
            items,
            comments: comments(source),
            tree,
            line_ending: line_ending(source),
        })
    }

    fn header(&self, path: &[Seg]) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.is_header() && item.path == path)
    }

    fn statement(&self, path: &[Seg]) -> Option<usize> {
        self.items
            .iter()
            .position(|item| !item.is_header() && item.path == path)
    }

    fn section_path(&self, section: Option<usize>) -> &[Seg] {
        section.map_or(&[], |index| &self.items[index].path)
    }

    fn statement_section(&self, index: usize) -> Option<usize> {
        match self.items[index].kind {
            Kind::Statement { section, .. } => section,
            Kind::Header { .. } => Some(index),
        }
    }

    /// The last item belonging to `section`: its last statement, else the
    /// header itself.
    fn section_last(&self, section: usize) -> usize {
        self.items
            .iter()
            .enumerate()
            .rev()
            .find(|(_, item)| {
                matches!(item.kind, Kind::Statement { section: Some(owner), .. } if owner == section)
            })
            .map_or(section, |(index, _)| index)
    }

    /// Where `header`'s section ends: the next header's leading comment
    /// block, or the end of the file.
    fn section_end(&self, header: usize) -> usize {
        self.items[header + 1..]
            .iter()
            .find(|item| item.is_header())
            .map_or(self.source.len(), |item| item.lead)
    }

    fn has_comment_in(&self, range: Range<usize>) -> bool {
        self.comments.iter().any(|comment| {
            range.start <= comment.start && comment.end <= range.end
        })
    }

    /// The indentation of the line that holds `item`.
    fn indent(&self, item: usize) -> &'a str {
        let line = &self.source[self.items[item].lines.clone()];
        &line[..line.len() - line.trim_start().len()]
    }
}

enum RawKind {
    Header { array: bool },
    Statement { value: Range<usize> },
}

type RawItem = (usize, usize, Vec<Seg>, RawKind);

fn walk(
    table: &toml_edit::Table,
    path: &mut Vec<Seg>,
    out: &mut Vec<RawItem>,
) -> Result<(), ApplyError> {
    let unspanned = || {
        ApplyError::UnsupportedLayout(
            "the parser reported an item without a source position".into(),
        )
    };
    for (name, item) in table {
        path.push(Seg::Key(name.to_owned()));
        match item {
            toml_edit::Item::Value(value) => {
                let key = table
                    .key(name)
                    .and_then(toml_edit::Key::span)
                    .ok_or_else(unspanned)?;
                let value = value.span().ok_or_else(unspanned)?;
                out.push((
                    key.start,
                    value.end,
                    path.clone(),
                    RawKind::Statement { value },
                ));
            }
            toml_edit::Item::Table(child) => {
                if !child.is_dotted() && !child.is_implicit() {
                    let span = child.span().ok_or_else(unspanned)?;
                    out.push((
                        span.start,
                        span.end,
                        path.clone(),
                        RawKind::Header { array: false },
                    ));
                }
                walk(child, path, out)?;
            }
            toml_edit::Item::ArrayOfTables(array) => {
                for (index, child) in array.iter().enumerate() {
                    path.push(Seg::Index(index));
                    let span = child.span().ok_or_else(unspanned)?;
                    out.push((
                        span.start,
                        span.end,
                        path.clone(),
                        RawKind::Header { array: true },
                    ));
                    walk(child, path, out)?;
                    path.pop();
                }
            }
            toml_edit::Item::None => {}
        }
        path.pop();
    }
    Ok(())
}

fn line_start(source: &str, position: usize) -> usize {
    source[..position].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(source: &str, position: usize) -> usize {
    source[position..]
        .find('\n')
        .map_or(source.len(), |index| position + index + 1)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The start of the contiguous comment lines directly above `start` that
/// begin after `floor`. A `#:` directive, such as `#:schema`, belongs to
/// the file rather than the item below it, so it ends the block.
fn leading_block(source: &str, start: usize, floor: usize) -> usize {
    let mut lead = start;
    while lead > floor {
        let previous = line_start(source, lead - 1);
        let line = source[previous..lead].trim_start();
        if previous < floor || !line.starts_with('#') || line.starts_with("#:")
        {
            break;
        }
        lead = previous;
    }
    lead
}

/// CRLF when every line ending is CRLF, LF otherwise.
fn line_ending(source: &str) -> &'static str {
    let lines = source.matches('\n').count();
    if lines > 0 && source.matches("\r\n").count() == lines {
        "\r\n"
    } else {
        "\n"
    }
}

/// Every comment's range, from `#` to the end of its line, skipping `#`
/// inside strings and quoted keys.
pub(super) fn comments(source: &str) -> Vec<Range<usize>> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'#' => {
                let end = source[index..]
                    .find('\n')
                    .map_or(source.len(), |offset| index + offset);
                let end = if end > index && bytes[end - 1] == b'\r' {
                    end - 1
                } else {
                    end
                };
                found.push(index..end);
                index = end;
            }
            quote @ (b'"' | b'\'') => {
                index = skip_string(bytes, index, quote);
            }
            _ => index += 1,
        }
    }
    found
}

/// The index just past the string that opens at `start`.
fn skip_string(bytes: &[u8], start: usize, quote: u8) -> usize {
    let triple = bytes[start..].starts_with(&[quote; 3]);
    let mut index = start + if triple { 3 } else { 1 };
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\\' && quote == b'"' {
            index += 2;
        } else if triple && bytes[index..].starts_with(&[quote; 3]) {
            // Up to two quotes may sit just inside the closing delimiter.
            let mut end = index + 3;
            while end < bytes.len() && bytes[end] == quote && end < index + 5 {
                end += 1;
            }
            return end;
        } else if !triple && byte == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    bytes.len()
}

/// Non-overlapping replacements of ranges in one text.
#[derive(Debug, Default)]
pub(super) struct Edits {
    edits: Vec<Edit>,
    /// Grouping tables the edits remove along with their last key.
    dropped: Vec<Vec<Seg>>,
}

#[derive(Debug)]
struct Edit {
    range: Range<usize>,
    text: String,
    /// Whether comments in `range` may disappear.
    removal: bool,
}

impl Edits {
    fn replace(&mut self, range: Range<usize>, text: String) {
        self.edits.push(Edit {
            range,
            text,
            removal: false,
        });
    }

    fn insert(&mut self, at: usize, text: String) {
        self.replace(at..at, text);
    }

    fn remove(&mut self, range: Range<usize>) {
        self.edits.push(Edit {
            range,
            text: String::new(),
            removal: true,
        });
    }

    pub(super) fn apply(&self, source: &str) -> String {
        let mut edits: Vec<_> = self.edits.iter().collect();
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut output = String::with_capacity(source.len());
        let mut copied = 0;
        for edit in edits {
            debug_assert!(edit.range.start >= copied, "edits overlap");
            output.push_str(&source[copied..edit.range.start]);
            output.push_str(&edit.text);
            copied = edit.range.end;
        }
        output.push_str(&source[copied..]);
        output
    }

    fn removed(&self) -> impl Iterator<Item = &Range<usize>> {
        self.edits
            .iter()
            .filter(|edit| edit.removal)
            .map(|edit| &edit.range)
    }
}

/// Renders `op` against `layout`.
pub(super) fn splice(
    layout: &Layout<'_>,
    op: &Op,
) -> Result<Edits, ApplyError> {
    match op {
        Op::Set(path, value) => set(layout, path, value),
        Op::Remove(path) => remove(layout, path),
        Op::AppendEntry(list, entry) => append_entry(layout, *list, entry),
        Op::RemoveEntry(list, index) => remove_entry(layout, *list, *index),
    }
}

fn unsupported(message: impl Into<String>) -> ApplyError {
    ApplyError::UnsupportedLayout(message.into())
}

pub(super) fn display(path: &[Seg]) -> String {
    let mut text = String::new();
    for seg in path {
        match seg {
            Seg::Key(key) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(&render_key(key));
            }
            Seg::Index(index) => {
                let _ = write!(text, "[{index}]");
            }
        }
    }
    text
}

fn render_key(key: &str) -> String {
    toml_edit::Key::new(key).to_string()
}

/// Keys joined for a statement or header; `path` holds no indices.
fn render_keys(path: &[Seg]) -> Result<String, ApplyError> {
    path.iter()
        .map(|seg| {
            seg.key().map(render_key).ok_or_else(|| {
                unsupported("an array entry cannot be written as a key")
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|keys| keys.join("."))
}

fn render_value(value: &Value) -> String {
    value.to_string()
}

/// Inserts one line after the item whose lines end at `at`, keeping a file
/// without a final newline that way.
fn insert_line(layout: &Layout<'_>, edits: &mut Edits, at: usize, line: &str) {
    let ending = layout.line_ending;
    if at == layout.source.len() && !layout.source.ends_with('\n') {
        edits.insert(at, format!("{ending}{line}"));
    } else {
        edits.insert(at, format!("{line}{ending}"));
    }
}

/// Inserts a block of lines that starts a new section at `at`, a line start
/// or the end of the file, separated from its neighbours by blank lines.
fn insert_block(
    layout: &Layout<'_>,
    edits: &mut Edits,
    at: usize,
    lines: &[String],
) {
    let ending = layout.line_ending;
    let source = layout.source;
    let body = lines.join(ending);
    if source.trim().is_empty() && at == source.len() {
        edits.insert(at, format!("{body}{ending}"));
    } else if at == source.len() {
        if source.ends_with('\n') {
            let separator = if source.ends_with(&format!("{ending}{ending}")) {
                ""
            } else {
                ending
            };
            edits.insert(at, format!("{separator}{body}{ending}"));
        } else {
            edits.insert(at, format!("{ending}{ending}{body}"));
        }
    } else {
        let before =
            if at == 0 || is_blank(&source[line_start(source, at - 1)..at]) {
                ""
            } else {
                ending
            };
        edits.insert(at, format!("{before}{body}{ending}{ending}"));
    }
}

/// The tree value at `path`, through tables and arrays.
pub(super) fn lookup<'t>(tree: &'t Table, path: &[Seg]) -> Option<&'t Value> {
    let (first, rest) = path.split_first()?;
    let mut value = tree.get(first.key()?)?;
    for seg in rest {
        value = match (seg, value) {
            (Seg::Key(key), Value::Table(table)) => table.get(key)?,
            (Seg::Index(index), Value::Array(array)) => array.get(*index)?,
            _ => return None,
        };
    }
    Some(value)
}

/// Checks that every ancestor of `path` is a table or absent.
fn check_ancestors(tree: &Table, path: &[Seg]) -> Result<(), ApplyError> {
    for end in 1..path.len() {
        match lookup(tree, &path[..end]) {
            None | Some(Value::Table(_)) => {}
            Some(Value::Array(_)) if matches!(path[end], Seg::Index(_)) => {}
            Some(other) => {
                return Err(unsupported(format!(
                    "{} is a {}, not a table",
                    display(&path[..end]),
                    other.type_str()
                )));
            }
        }
    }
    Ok(())
}

/// A statement whose value contains `path`, such as an inline table.
fn enclosing_statement(layout: &Layout<'_>, path: &[Seg]) -> Option<usize> {
    layout.items.iter().position(|item| {
        !item.is_header()
            && item.path.len() < path.len()
            && path.starts_with(&item.path)
    })
}

fn set(
    layout: &Layout<'_>,
    path: &[Seg],
    value: &Value,
) -> Result<Edits, ApplyError> {
    check_ancestors(&layout.tree, path)?;
    let mut edits = Edits::default();
    if let Some(index) = layout.statement(path) {
        let Kind::Statement { value: span, .. } = &layout.items[index].kind
        else {
            unreachable!("statement() returns statements");
        };
        if layout.has_comment_in(span.clone()) {
            return Err(unsupported(format!(
                "{} holds comments; edit it in the file",
                display(path)
            )));
        }
        edits.replace(span.clone(), render_value(value));
        return Ok(edits);
    }
    if let Some(index) = enclosing_statement(layout, path) {
        edit_inline(layout, index, path, Some(value), &mut edits)?;
        return Ok(edits);
    }
    if layout.header(path).is_some() {
        return Err(unsupported(format!(
            "{} is a table section",
            display(path)
        )));
    }
    insert(layout, path, value, &mut edits)?;
    Ok(edits)
}

/// Adds a key that does not exist yet.
fn insert(
    layout: &Layout<'_>,
    path: &[Seg],
    value: &Value,
    edits: &mut Edits,
) -> Result<(), ApplyError> {
    let rendered = render_value(value);
    let parent = &path[..path.len() - 1];
    // The nearest ancestor that the file writes as a section or through
    // dotted keys decides where the key goes.
    for end in (0..=parent.len()).rev() {
        let ancestor = &path[..end];
        if end == 0 {
            if parent.is_empty() {
                let key = render_keys(path)?;
                insert_root(layout, &format!("{key} = {rendered}"), edits);
                return Ok(());
            }
            break;
        }
        if let Some(header) = layout.header(ancestor) {
            let inside_entry =
                ancestor.iter().any(|seg| matches!(seg, Seg::Index(_)));
            if end == parent.len() || inside_entry {
                // A missing table inside an array entry stays in the entry's
                // own section as a dotted key, so later entries keep it.
                let key = render_keys(&path[end..])?;
                let anchor = layout.section_last(header);
                let indent = if anchor == header {
                    ""
                } else {
                    layout.indent(anchor)
                };
                insert_line(
                    layout,
                    edits,
                    layout.items[anchor].lines.end,
                    &format!("{indent}{key} = {rendered}"),
                );
                return Ok(());
            }
            break;
        }
        if let Some(anchor) = last_dotted(layout, ancestor) {
            let section = layout.statement_section(anchor);
            let key = render_keys(&path[layout.section_path(section).len()..])?;
            let indent = layout.indent(anchor);
            insert_line(
                layout,
                edits,
                layout.items[anchor].lines.end,
                &format!("{indent}{key} = {rendered}"),
            );
            return Ok(());
        }
    }
    // A new section, before the first array of tables so entries stay last.
    // A table of plain values becomes its own section, such as
    // `[quake.profiles.logs]`; anything else is a key in its parent's.
    let at = layout
        .items
        .iter()
        .find(|item| matches!(item.kind, Kind::Header { array: true }))
        .map_or(layout.source.len(), |item| item.lead);
    if let Value::Table(table) = value
        && !table
            .values()
            .any(|value| matches!(value, Value::Table(_) | Value::Array(_)))
    {
        let mut lines = vec![format!("[{}]", render_keys(path)?)];
        for (key, value) in table {
            lines.push(format!(
                "{} = {}",
                render_key(key),
                render_value(value)
            ));
        }
        insert_block(layout, edits, at, &lines);
        return Ok(());
    }
    let header = format!("[{}]", render_keys(parent)?);
    let key = render_keys(&path[parent.len()..])?;
    insert_block(layout, edits, at, &[header, format!("{key} = {rendered}")]);
    Ok(())
}

/// The last statement that writes into `table` through dotted keys.
fn last_dotted(layout: &Layout<'_>, table: &[Seg]) -> Option<usize> {
    layout
        .items
        .iter()
        .enumerate()
        .rev()
        .find(|(index, item)| {
            !item.is_header()
                && item.path.len() > table.len()
                && item.path.starts_with(table)
                && layout.section_path(layout.statement_section(*index)).len()
                    < table.len()
        })
        .map(|(index, _)| index)
}

fn insert_root(layout: &Layout<'_>, line: &str, edits: &mut Edits) {
    let last_root = layout.items.iter().enumerate().rev().find(|(_, item)| {
        matches!(item.kind, Kind::Statement { section: None, .. })
    });
    if let Some((index, _)) = last_root {
        insert_line(layout, edits, layout.items[index].lines.end, line);
    } else if let Some(first) =
        layout.items.iter().find(|item| item.is_header())
    {
        let ending = layout.line_ending;
        edits.insert(first.lead, format!("{line}{ending}{ending}"));
    } else {
        insert_block(layout, edits, layout.source.len(), &[line.to_owned()]);
    }
}

/// Re-renders the inline table that statement `index` holds with the value
/// at `path` set, or removed when `value` is `None`.
fn edit_inline(
    layout: &Layout<'_>,
    index: usize,
    path: &[Seg],
    value: Option<&Value>,
    edits: &mut Edits,
) -> Result<(), ApplyError> {
    let item = &layout.items[index];
    let Kind::Statement { value: span, .. } = &item.kind else {
        unreachable!("enclosing_statement returns statements");
    };
    let source = &layout.source[span.clone()];
    if layout.has_comment_in(span.clone()) {
        return Err(unsupported(format!(
            "{} is a multiline inline table with comments; edit it in the file",
            display(&item.path)
        )));
    }
    let mut parsed: toml_edit::Value =
        source.parse().map_err(|error: toml_edit::TomlError| {
            unsupported(error.to_string())
        })?;
    let relative = &path[item.path.len()..];
    let mut table = match &mut parsed {
        toml_edit::Value::InlineTable(table) => table,
        other => {
            return Err(unsupported(format!(
                "{} is a {}, not a table",
                display(&item.path),
                other.type_name()
            )));
        }
    };
    let (leaf, parents) =
        relative.split_last().expect("path is below the statement");
    for seg in parents {
        let key = seg.key().ok_or_else(|| {
            unsupported("arrays inside inline tables are not editable")
        })?;
        if value.is_some() && !table.contains_key(key) {
            table.insert(
                key,
                toml_edit::Value::InlineTable(toml_edit::InlineTable::new()),
            );
        }
        table = match table.get_mut(key) {
            Some(toml_edit::Value::InlineTable(child)) => child,
            Some(other) => {
                return Err(unsupported(format!(
                    "{key} is a {}, not a table",
                    other.type_name()
                )));
            }
            None => return Ok(()),
        };
    }
    let leaf = leaf.key().ok_or_else(|| {
        unsupported("arrays inside inline tables are not editable")
    })?;
    if let Some(value) = value {
        let mut rendered: toml_edit::Value =
            render_value(value).parse().map_err(
                |error: toml_edit::TomlError| unsupported(error.to_string()),
            )?;
        if let Some(existing) = table.get_mut(leaf) {
            *rendered.decor_mut() = existing.decor().clone();
            *existing = rendered;
        } else {
            // The space before `}` moves from the old last field to the
            // new one.
            if let Some((_, last)) = table.iter_mut().last()
                && let Some(suffix) = last.decor().suffix().cloned()
            {
                last.decor_mut().set_suffix("");
                rendered.decor_mut().set_suffix(suffix);
            }
            table.insert(leaf, rendered);
        }
    } else {
        let last = table.iter().last().map(|(key, _)| key.to_owned());
        let Some(removed) = table.remove(leaf) else {
            return Ok(());
        };
        // The last field's suffix holds the space before `}`; it moves
        // to the new last field.
        if last.as_deref() == Some(leaf)
            && let Some(suffix) = removed.decor().suffix().cloned()
            && let Some((_, value)) = table.iter_mut().last()
        {
            value.decor_mut().set_suffix(suffix);
        }
    }
    edits.replace(span.clone(), parsed.to_string());
    Ok(())
}

fn remove(layout: &Layout<'_>, path: &[Seg]) -> Result<Edits, ApplyError> {
    let mut edits = Edits::default();
    if let Some(index) = layout.statement(path) {
        remove_statement(layout, index, &mut edits);
        return Ok(edits);
    }
    if let Some(index) = enclosing_statement(layout, path) {
        edit_inline(layout, index, path, None, &mut edits)?;
        return Ok(edits);
    }
    // A field of an entry written as its own sections, such as
    // `[keybinding.args]`, goes as the entry's subtables do when it goes.
    if path.iter().any(|seg| matches!(seg, Seg::Index(_))) {
        let ranges = owned_ranges(layout, path);
        if !ranges.is_empty() {
            return Ok(remove_ranges(layout, ranges));
        }
    }
    // A table written as its own sections, such as `[quake.profiles.logs]`,
    // goes whole with the dotted keys below it, and so do the parents only
    // it defined.
    if let Some(Value::Table(_)) = lookup(&layout.tree, path)
        && !path.iter().any(|seg| matches!(seg, Seg::Index(_)))
    {
        let ranges = owned_ranges(layout, path);
        if !ranges.is_empty() {
            let mut edits = remove_ranges(layout, ranges);
            let mut tree = layout.tree.clone();
            remove_at(&mut tree, path);
            for end in (1..path.len()).rev() {
                let parent = &path[..end];
                let empty = matches!(
                    lookup(&tree, parent),
                    Some(Value::Table(parent)) if parent.is_empty()
                );
                let defined = layout.items.iter().any(|item| {
                    item.path.starts_with(parent)
                        && !item.path.starts_with(path)
                });
                if !empty || defined || !is_grouping(parent) {
                    break;
                }
                edits.dropped.push(parent.to_vec());
                remove_at(&mut tree, parent);
            }
            return Ok(edits);
        }
    }
    if lookup(&layout.tree, path).is_some() {
        return Err(unsupported(format!(
            "{} is a table section, not a value",
            display(path)
        )));
    }
    Ok(edits)
}

/// Removes a statement and its leading comments, then whatever the removal
/// leaves empty: a grouping table goes when nothing comments it, and any
/// other table stays, written as `{}` once nothing else defines it.
fn remove_statement(layout: &Layout<'_>, index: usize, edits: &mut Edits) {
    let item = &layout.items[index];
    let section = layout.statement_section(index);
    let section_len = layout.section_path(section).len();
    let mut tree = layout.tree.clone();
    remove_at(&mut tree, &item.path);
    let mut removal = item.lead..item.lines.end;
    let mut header_dropped = false;
    for end in (1..item.path.len()).rev() {
        let table = &item.path[..end];
        if !matches!(lookup(&tree, table), Some(Value::Table(table)) if table.is_empty())
        {
            break;
        }
        if end > section_len {
            // Only this statement's dotted keys defined the table.
            if is_grouping(table) {
                edits.dropped.push(table.to_vec());
                remove_at(&mut tree, table);
                continue;
            }
            // Only the key through the value changes, so the statement's
            // indentation and trailing comment stay.
            let keys = render_keys(&item.path[section_len..end])
                .expect("statement keys hold no indices");
            let Kind::Statement { value, .. } = &item.kind else {
                unreachable!("remove_statement takes statements");
            };
            let start = item.lines.start + layout.indent(index).len();
            edits.replace(start..value.end, format!("{keys} = {{}}"));
            return;
        }
        // The section's header defines the table; it goes only when this
        // was its last statement and nothing in it carries a comment.
        let Some(header) =
            section.filter(|&header| layout.items[header].path == table)
        else {
            break;
        };
        let header_item = &layout.items[header];
        let alone = layout.section_last(header) == index
            && !layout.items.iter().enumerate().any(|(other, candidate)| {
                other != index
                    && other != header
                    && candidate.path.starts_with(table)
            });
        if !is_grouping(table)
            || !alone
            || layout
                .has_comment_in(header_item.lead..layout.section_end(header))
        {
            break;
        }
        edits.dropped.push(table.to_vec());
        remove_at(&mut tree, table);
        removal.start = header_item.lead;
        header_dropped = true;
        // The header also defined its implicit parents, such as `terminal`
        // for `[terminal.bell]`; those left empty go with it.
        for end in (1..table.len()).rev() {
            let parent = &table[..end];
            let empty = matches!(lookup(&tree, parent), Some(Value::Table(parent)) if parent.is_empty());
            if !is_grouping(parent)
                || !empty
                || layout.items.iter().any(|item| item.path == parent)
            {
                break;
            }
            edits.dropped.push(parent.to_vec());
            remove_at(&mut tree, parent);
        }
        break;
    }
    if header_dropped {
        absorb_blank(layout.source, &mut removal);
    }
    edits.remove(removal);
}

/// Extends a removed section over one blank line that separated it from its
/// neighbours: the one before it, else the one after it.
fn absorb_blank(source: &str, range: &mut Range<usize>) {
    if range.start > 0 {
        let previous = line_start(source, range.start - 1);
        if is_blank(&source[previous..range.start]) {
            range.start = previous;
            return;
        }
    }
    if range.end < source.len() {
        let next = line_end(source, range.end);
        if is_blank(&source[range.end..next]) {
            range.end = next;
        }
    }
}

pub(super) fn remove_at(tree: &mut Table, path: &[Seg]) -> Option<Value> {
    let (last, parents) = path.split_last()?;
    if parents.is_empty() {
        return tree.remove(last.key()?);
    }
    let mut value = tree.get_mut(parents[0].key()?)?;
    for seg in &parents[1..] {
        value = match (seg, value) {
            (Seg::Key(key), Value::Table(table)) => table.get_mut(key)?,
            (Seg::Index(index), Value::Array(array)) => {
                array.get_mut(*index)?
            }
            _ => return None,
        };
    }
    match (last, value) {
        (Seg::Key(key), Value::Table(table)) => table.remove(key),
        (Seg::Index(index), Value::Array(array)) if *index < array.len() => {
            Some(array.remove(*index))
        }
        _ => None,
    }
}

pub(super) fn insert_at(tree: &mut Table, path: &[Seg], value: Value) -> bool {
    let Some((Seg::Key(last), parents)) = path.split_last() else {
        return false;
    };
    let mut table = tree;
    let mut index = 0;
    while index < parents.len() {
        let Seg::Key(key) = &parents[index] else {
            return false;
        };
        index += 1;
        let slot = table
            .entry(key.clone())
            .or_insert_with(|| Value::Table(Table::new()));
        table = match slot {
            Value::Table(child) => child,
            Value::Array(entries) => {
                let Some(Seg::Index(entry)) = parents.get(index) else {
                    return false;
                };
                index += 1;
                match entries.get_mut(*entry) {
                    Some(Value::Table(child)) => child,
                    _ => return false,
                }
            }
            _ => return false,
        };
    }
    table.insert(last.clone(), value);
    true
}

fn list_items(
    layout: &Layout<'_>,
    list: List,
) -> Result<Vec<usize>, ApplyError> {
    let name = list.key();
    if layout.statement(&[Seg::Key(name.into())]).is_some() {
        return Err(unsupported(format!(
            "{name} is written as an inline array; edit it in the file"
        )));
    }
    Ok(layout
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.path.first().and_then(Seg::key) == Some(name))
        .map(|(index, _)| index)
        .collect())
}

fn append_entry(
    layout: &Layout<'_>,
    list: List,
    entry: &Table,
) -> Result<Edits, ApplyError> {
    let owned = list_items(layout, list)?;
    let mut lines = vec![format!("[[{}]]", list.key())];
    let mut fields: Vec<(&String, &Value)> = Vec::with_capacity(entry.len());
    for name in ENTRY_FIELDS {
        if let Some((key, value)) = entry.get_key_value(*name) {
            fields.push((key, value));
        }
    }
    for (key, value) in entry {
        if !ENTRY_FIELDS.contains(&key.as_str()) {
            fields.push((key, value));
        }
    }
    for (key, value) in fields {
        lines.push(format!("{} = {}", render_key(key), render_value(value)));
    }
    let mut edits = Edits::default();
    match owned.last() {
        Some(&final_item) => {
            let ending = layout.line_ending;
            let at = layout.items[final_item].lines.end;
            let body = lines.join(ending);
            if at == layout.source.len() && !layout.source.ends_with('\n') {
                edits.insert(at, format!("{ending}{ending}{body}"));
            } else {
                edits.insert(at, format!("{ending}{body}{ending}"));
            }
        }
        None => insert_block(layout, &mut edits, layout.source.len(), &lines),
    }
    Ok(edits)
}

fn remove_entry(
    layout: &Layout<'_>,
    list: List,
    index: usize,
) -> Result<Edits, ApplyError> {
    list_items(layout, list)?;
    let ranges =
        owned_ranges(layout, &[Seg::Key(list.key().into()), Seg::Index(index)]);
    if ranges.is_empty() {
        return Err(ApplyError::Conflict(vec![format!(
            "{} has no entry {}",
            list.key(),
            index + 1
        )]));
    }
    Ok(remove_ranges(layout, ranges))
}

/// The source ranges that define `path` and everything below it: each
/// section whose header is at or below `path`, through the next header's
/// leading comment block, and each statement elsewhere that writes below
/// it, each with its leading comments.
fn owned_ranges(layout: &Layout<'_>, path: &[Seg]) -> Vec<Range<usize>> {
    layout
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.path.starts_with(path))
        .map(|(index, item)| {
            if item.is_header() {
                item.lead..layout.section_end(index)
            } else {
                item.lead..item.lines.end
            }
        })
        .collect()
}

/// Removes `ranges`, merged where they overlap, each with at most one
/// adjacent blank separator line.
fn remove_ranges(layout: &Layout<'_>, mut ranges: Vec<Range<usize>>) -> Edits {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(previous) if previous.end >= range.start => {
                previous.end = previous.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }
    let source = layout.source;
    let mut edits = Edits::default();
    for mut range in merged {
        let ends_blank = {
            let text = &source[range.clone()];
            let body = text.trim_end_matches(['\r', '\n']);
            text[body.len()..].matches('\n').count() > 1
        };
        if (!ends_blank || range.end == source.len()) && range.start > 0 {
            let previous = line_start(source, range.start - 1);
            if is_blank(&source[previous..range.start])
                && edits.edits.iter().all(|edit| edit.range.end <= previous)
            {
                range.start = previous;
            }
        }
        edits.remove(range);
    }
    edits
}

/// The value tree `op` should produce from `tree`, with the grouping tables
/// the splice reports as dropped removed once empty.
fn expected(tree: &Table, op: &Op, edits: &Edits) -> Table {
    let mut tree = tree.clone();
    match op {
        Op::Set(path, value) => {
            insert_at(&mut tree, path, value.clone());
        }
        Op::Remove(path) => {
            remove_at(&mut tree, path);
        }
        Op::AppendEntry(list, entry) => {
            let slot = tree
                .entry(list.key().to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(entries) = slot {
                entries.push(Value::Table(entry.clone()));
            }
        }
        Op::RemoveEntry(list, index) => {
            let path = [Seg::Key(list.key().into()), Seg::Index(*index)];
            remove_at(&mut tree, &path);
            if matches!(tree.get(list.key()), Some(Value::Array(entries)) if entries.is_empty())
            {
                tree.remove(list.key());
            }
        }
    }
    for table in &edits.dropped {
        if is_grouping(table)
            && matches!(lookup(&tree, table), Some(Value::Table(table)) if table.is_empty())
        {
            remove_at(&mut tree, table);
        }
    }
    tree
}

/// The semantic delta and comment guards for one applied operation.
///
/// # Errors
/// [`ApplyError::Guard`] when the new text's values differ from the
/// expected tree in any way, or a comment outside the removed ranges went
/// missing. Either means a splice bug, so nothing is written.
pub(super) fn verify(
    layout: &Layout<'_>,
    op: &Op,
    edits: &Edits,
    output: &str,
) -> Result<(), ApplyError> {
    let actual: Table = toml::from_str(output).map_err(|error| {
        ApplyError::Guard(format!("the edited file does not parse: {error}"))
    })?;
    let expected = expected(&layout.tree, op, edits);
    if !same_table(&actual, &expected) {
        return Err(ApplyError::Guard(
            "the edit changed values it was not asked to change".into(),
        ));
    }
    let mut remaining: HashMap<&str, usize> = HashMap::new();
    for comment in comments(output) {
        *remaining.entry(&output[comment]).or_default() += 1;
    }
    for comment in &layout.comments {
        if edits.removed().any(|range| {
            range.start <= comment.start && comment.end <= range.end
        }) {
            continue;
        }
        let text = &layout.source[comment.clone()];
        match remaining.get_mut(text) {
            Some(count) if *count > 0 => *count -= 1,
            _ => {
                return Err(ApplyError::Guard(format!(
                    "the edit lost the comment {text:?}"
                )));
            }
        }
    }
    Ok(())
}

fn same_table(left: &Table, right: &Table) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, value)| {
            right.get(key).is_some_and(|other| same_value(value, other))
        })
}

/// Value equality that treats NaN as equal to itself.
fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Float(left), Value::Float(right)) => {
            left.to_bits() == right.to_bits()
                || (left.is_nan() && right.is_nan())
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_value(left, right))
        }
        (Value::Table(left), Value::Table(right)) => same_table(left, right),
        _ => left == right,
    }
}

/// Whether two values are equal for rebasing and locating entries.
pub(super) fn same(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => same_value(left, right),
        _ => false,
    }
}
