//! Comment-preserving changes to the configuration file.
//!
//! [`ConfigFile`] reads the file into a [`Snapshot`] and writes [`Patch`]es
//! back without disturbing anything they do not change: other values,
//! comments, spacing, and line endings stay byte for byte. A patch made
//! against an older snapshot rebases onto the current file when it touches
//! nothing that changed since, and fails with [`ApplyError::Conflict`]
//! otherwise, so an edit made in another editor is never overwritten.
//!
//! Callers serialize [`ConfigFile::apply`] through the owner of the
//! `ConfigFile`, one writer per process. Another process can still write
//! between the final recheck and the rename; that window is accepted.

mod splice;

use std::fmt;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use toml::{Table, Value};

use splice::{Layout, Op, Seg};

/// A temporary file older than this belongs to a crashed save.
const STALE_TEMPORARY: Duration = Duration::from_mins(10);

/// Which list of `[[...]]` entries an entry patch addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum List {
    Keybinding,
    GlobalKeybinding,
}

impl List {
    const fn key(self) -> &'static str {
        match self {
            Self::Keybinding => "keybinding",
            Self::GlobalKeybinding => "global_keybinding",
        }
    }
}

/// One entry of a list as a snapshot held it: its position and every field.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryLocator {
    list: List,
    index: usize,
    content: Table,
}

impl EntryLocator {
    /// The entry's fields when it was located.
    #[must_use]
    pub fn content(&self) -> &Table {
        &self.content
    }

    /// Finds the entry in `tree`: the one at the same index when its content
    /// is unchanged, else the only entry anywhere with that exact content.
    fn resolve(&self, tree: &Table) -> Result<usize, String> {
        let entries = match tree.get(self.list.key()) {
            Some(Value::Array(entries)) => entries.as_slice(),
            _ => &[],
        };
        let content = Value::Table(self.content.clone());
        let matches = |entry: &Value| splice::same(Some(entry), Some(&content));
        if entries.get(self.index).is_some_and(matches) {
            return Ok(self.index);
        }
        let mut found = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches(entry))
            .map(|(index, _)| index);
        match (found.next(), found.next()) {
            (Some(index), None) => Ok(index),
            (None, _) => Err(format!(
                "{} entry {} changed or was removed",
                self.list.key(),
                self.index + 1
            )),
            (Some(_), Some(_)) => Err(format!(
                "{} entry {} now matches several entries",
                self.list.key(),
                self.index + 1
            )),
        }
    }
}

/// One change to the file.
#[derive(Clone, Debug, PartialEq)]
pub enum Patch {
    /// Sets the value at a dotted path such as `["font", "size"]`. A table
    /// value merges key by key into a table that already exists.
    Set { path: Vec<String>, value: Value },
    /// Removes the value at a path, which resets it to its default.
    Remove { path: Vec<String> },
    /// Appends an entry to a list.
    AppendEntry { list: List, entry: Table },
    /// Replaces a located entry's fields, editing only those that differ.
    UpdateEntry { locator: EntryLocator, entry: Table },
    /// Sets one field of a located entry.
    SetEntryField {
        locator: EntryLocator,
        field: String,
        value: Value,
    },
    /// Removes one field of a located entry.
    ClearEntryField {
        locator: EntryLocator,
        field: String,
    },
    /// Removes a located entry with its comments and subtables.
    RemoveEntry { locator: EntryLocator },
}

impl Patch {
    fn locator(&self) -> Option<&EntryLocator> {
        match self {
            Self::Set { .. }
            | Self::Remove { .. }
            | Self::AppendEntry { .. } => None,
            Self::UpdateEntry { locator, .. }
            | Self::SetEntryField { locator, .. }
            | Self::ClearEntryField { locator, .. }
            | Self::RemoveEntry { locator } => Some(locator),
        }
    }
}

/// What a snapshot read: the file's content hash, the target it resolved
/// to, and that target's identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revision {
    content: Option<u64>,
    target: PathBuf,
    identity: Option<(u64, u64)>,
}

impl Revision {
    /// Whether the file was absent.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        self.content.is_none()
    }
}

/// The file's text when it was read, and its revision.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    text: Option<String>,
    revision: Revision,
}

impl Snapshot {
    /// The file's text, or `None` when it did not exist.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    #[must_use]
    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    /// A locator for entry `index` of `list`, if the snapshot has one.
    #[must_use]
    pub fn entry(&self, list: List, index: usize) -> Option<EntryLocator> {
        let tree: Table = toml::from_str(self.text.as_deref()?).ok()?;
        match tree.get(list.key())? {
            Value::Array(entries) => match entries.get(index)? {
                Value::Table(content) => Some(EntryLocator {
                    list,
                    index,
                    content: content.clone(),
                }),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum ApplyError {
    /// The file on disk does not parse or fails validation, so nothing can
    /// be applied to it.
    InvalidFile(String),
    /// The validator rejected the result.
    Rejected(String),
    /// The file changed where the patches apply; each string names one.
    Conflict(Vec<String>),
    /// The file writes a setting in a form the service does not edit.
    UnsupportedLayout(String),
    /// The configuration path is a symlink to nothing.
    DanglingSymlink(PathBuf),
    /// An edit would have changed something it was not asked to; a bug.
    Guard(String),
    /// A failure before the file was replaced; the original is intact.
    Io(io::Error),
    /// The file was replaced, but making that durable failed. Refresh the
    /// revision from disk; nothing is rolled back.
    WrittenNotDurable(io::Error),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFile(message) => {
                write!(
                    formatter,
                    "the configuration file is invalid: {message}"
                )
            }
            Self::Conflict(changes) => write!(
                formatter,
                "the configuration file changed on disk: {}",
                changes.join("; ")
            ),
            Self::Rejected(message) | Self::UnsupportedLayout(message) => {
                formatter.write_str(message)
            }
            Self::DanglingSymlink(path) => write!(
                formatter,
                "{} is a symlink to a missing file",
                path.display()
            ),
            Self::Guard(message) => {
                write!(
                    formatter,
                    "refused to write the configuration: {message}"
                )
            }
            Self::Io(error) => error.fmt(formatter),
            Self::WrittenNotDurable(error) => write!(
                formatter,
                "the configuration was saved but may not survive a crash: {error}"
            ),
        }
    }
}

impl std::error::Error for ApplyError {}

/// A point in [`ConfigFile::apply`] where tests inject faults and barriers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    TempWrite,
    TempSync,
    Recheck,
    Rename,
    DirectorySync,
    ParentSync,
}

/// Runs before each stage; the production hooks do nothing.
trait Hooks {
    fn before(&self, stage: Stage) -> io::Result<()>;
}

struct NoHooks;

impl Hooks for NoHooks {
    fn before(&self, _: Stage) -> io::Result<()> {
        Ok(())
    }
}

/// The configuration file at its logical path, the path the application
/// reads. Writes go to the file that path resolves to, so a symlink stays.
#[derive(Clone, Debug)]
pub struct ConfigFile {
    logical: PathBuf,
}

/// Where the logical path leads and what is there.
#[derive(Debug, Eq, PartialEq)]
struct Target {
    path: PathBuf,
    identity: Option<(u64, u64)>,
    content: Option<Vec<u8>>,
}

impl Target {
    fn revision(&self) -> Revision {
        Revision {
            content: self.content.as_deref().map(hash),
            target: self.path.clone(),
            identity: self.identity,
        }
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl ConfigFile {
    #[must_use]
    pub fn new(logical: impl Into<PathBuf>) -> Self {
        Self {
            logical: logical.into(),
        }
    }

    #[must_use]
    pub fn logical_path(&self) -> &Path {
        &self.logical
    }

    /// Reads the file as it is now.
    ///
    /// # Errors
    /// Returns a read failure or a dangling symlink.
    pub fn load(&self) -> Result<Snapshot, ApplyError> {
        let target = self.resolve()?;
        snapshot(&target)
    }

    /// Applies `patches` to the file as it is now and writes the result.
    ///
    /// `validate` checks the current text and the candidate the same way;
    /// it must not have side effects. Returns the written snapshot.
    ///
    /// # Errors
    /// See [`ApplyError`]. Every error except
    /// [`ApplyError::WrittenNotDurable`] leaves the file unchanged.
    pub fn apply(
        &self,
        base: &Snapshot,
        patches: &[Patch],
        validate: &dyn Fn(&str) -> Result<(), String>,
    ) -> Result<Snapshot, ApplyError> {
        self.apply_with(base, patches, validate, &NoHooks)
    }

    fn apply_with(
        &self,
        base: &Snapshot,
        patches: &[Patch],
        validate: &dyn Fn(&str) -> Result<(), String>,
        hooks: &dyn Hooks,
    ) -> Result<Snapshot, ApplyError> {
        let mut restarted = false;
        // A restarted attempt finds the directories the first one created
        // already there, but they still need their entries made durable.
        let mut created = Vec::new();
        loop {
            let target = self.resolve()?;
            let current = snapshot(&target)?;
            if current.text.is_none() && !base.revision.is_absent() {
                return Err(ApplyError::Conflict(vec![format!(
                    "{} was removed",
                    self.logical.display()
                )]));
            }
            if let Some(text) = &current.text {
                validate(text).map_err(ApplyError::InvalidFile)?;
            }
            if current.revision != base.revision {
                rebase(base, &current, patches)?;
            }
            let candidate =
                render(current.text.as_deref().unwrap_or(""), patches)?;
            validate(&candidate).map_err(ApplyError::Rejected)?;
            match write(
                &self.logical,
                &target,
                &candidate,
                &mut created,
                hooks,
            )? {
                Written::Replaced(snapshot) => return Ok(snapshot),
                Written::Changed if restarted => {
                    return Err(ApplyError::Conflict(vec![format!(
                        "{} kept changing while saving",
                        self.logical.display()
                    )]));
                }
                Written::Changed => restarted = true,
            }
        }
    }

    fn resolve(&self) -> Result<Target, ApplyError> {
        resolve(&self.logical)
    }
}

fn snapshot(target: &Target) -> Result<Snapshot, ApplyError> {
    let text = target
        .content
        .as_ref()
        .map(|bytes| {
            String::from_utf8(bytes.clone()).map_err(|error| {
                ApplyError::InvalidFile(format!("not UTF-8: {error}"))
            })
        })
        .transpose()?;
    Ok(Snapshot {
        text,
        revision: target.revision(),
    })
}

/// Follows the logical path through symlinks and reads what it reaches.
fn resolve(logical: &Path) -> Result<Target, ApplyError> {
    match fs::symlink_metadata(logical) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Target {
            path: resolve_missing(logical),
            identity: None,
            content: None,
        }),
        Err(error) => Err(ApplyError::Io(error)),
        Ok(_) => {
            let path = fs::canonicalize(logical).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    ApplyError::DanglingSymlink(logical.to_owned())
                } else {
                    ApplyError::Io(error)
                }
            })?;
            let content = fs::read(&path).map_err(ApplyError::Io)?;
            let metadata = fs::metadata(&path).map_err(ApplyError::Io)?;
            Ok(Target {
                identity: identity(&metadata),
                path,
                content: Some(content),
            })
        }
    }
}

/// The path a missing file would have: its nearest existing ancestor,
/// canonicalized, joined with the rest.
fn resolve_missing(logical: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut ancestor = logical;
    while let Some(parent) = ancestor.parent() {
        rest.push(ancestor.file_name().unwrap_or_default().to_owned());
        if let Ok(canonical) = fs::canonicalize(parent) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |path, name| path.join(name));
        }
        ancestor = parent;
    }
    logical.to_owned()
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "other platforms have no file identity"
)]
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Checks that every patch still applies to `current`.
fn rebase(
    base: &Snapshot,
    current: &Snapshot,
    patches: &[Patch],
) -> Result<(), ApplyError> {
    let parse = |snapshot: &Snapshot| -> Option<Table> {
        snapshot
            .text
            .as_deref()
            .map_or(Some(Table::new()), |text| toml::from_str(text).ok())
    };
    let base_tree = parse(base);
    let current_tree = parse(current).unwrap_or_default();
    let mut conflicts = Vec::new();
    for patch in patches {
        let path = match patch {
            Patch::Set { path, .. } | Patch::Remove { path } => path,
            Patch::AppendEntry { .. } => continue,
            Patch::UpdateEntry { locator, .. }
            | Patch::SetEntryField { locator, .. }
            | Patch::ClearEntryField { locator, .. }
            | Patch::RemoveEntry { locator } => {
                if let Err(conflict) = locator.resolve(&current_tree) {
                    conflicts.push(conflict);
                }
                continue;
            }
        };
        let path = keys(path);
        let unchanged = base_tree.as_ref().is_some_and(|base_tree| {
            splice::same(
                splice::lookup(base_tree, &path),
                splice::lookup(&current_tree, &path),
            )
        });
        if !unchanged {
            conflicts.push(format!("{} changed", splice::display(&path)));
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(ApplyError::Conflict(conflicts))
    }
}

fn keys(path: &[String]) -> Vec<Seg> {
    path.iter().map(|key| Seg::Key(key.clone())).collect()
}

/// Applies `patches` to `source` in order, verifying every step.
///
/// Every locator resolves once, against `source`, so an entry keeps its
/// identity through the earlier patches' edits and removals.
fn render(source: &str, patches: &[Patch]) -> Result<String, ApplyError> {
    let tree = Layout::parse(source)?.tree;
    let mut conflicts = Vec::new();
    let located: Vec<Option<(List, usize)>> = patches
        .iter()
        .map(|patch| {
            let locator = patch.locator()?;
            match locator.resolve(&tree) {
                Ok(index) => Some((locator.list, index)),
                Err(conflict) => {
                    conflicts.push(conflict);
                    None
                }
            }
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(ApplyError::Conflict(conflicts));
    }
    let mut removed: Vec<(List, usize)> = Vec::new();
    let mut text = source.to_owned();
    for (patch, located) in patches.iter().zip(located) {
        let index = located
            .map(|(list, index)| current_index(&removed, list, index))
            .transpose()?;
        let layout = Layout::parse(&text)?;
        let ops = expand(&layout.tree, patch, index);
        for op in ops {
            text = apply_op(&text, &op)?;
        }
        if let (Patch::RemoveEntry { .. }, Some(located)) = (patch, located) {
            removed.push(located);
        }
    }
    Ok(text)
}

/// Where entry `index` of `list` in the source is after the entries in
/// `removed`, all source indices, are gone. Appended entries come after
/// every source entry, so they never move one.
fn current_index(
    removed: &[(List, usize)],
    list: List,
    index: usize,
) -> Result<usize, ApplyError> {
    if removed.contains(&(list, index)) {
        return Err(ApplyError::Conflict(vec![format!(
            "{} entry {} was removed by an earlier change",
            list.key(),
            index + 1
        )]));
    }
    let before = removed
        .iter()
        .filter(|&&(other, at)| other == list && at < index)
        .count();
    Ok(index - before)
}

fn apply_op(text: &str, op: &Op) -> Result<String, ApplyError> {
    let layout = Layout::parse(text)?;
    let edits = splice::splice(&layout, op)?;
    let output = edits.apply(text);
    splice::verify(&layout, op, &edits, &output)?;
    Ok(output)
}

/// Turns a patch into primitive operations against `tree`. An entry patch
/// addresses entry `index`, its located entry's position in `tree`.
fn expand(tree: &Table, patch: &Patch, index: Option<usize>) -> Vec<Op> {
    let located = || index.expect("entry patches arrive located");
    let mut ops = Vec::new();
    match patch {
        Patch::Set { path, value } => {
            set_ops(tree, keys(path), value, &mut ops);
        }
        Patch::Remove { path } => ops.push(Op::Remove(keys(path))),
        Patch::AppendEntry { list, entry } => {
            ops.push(Op::AppendEntry(*list, entry.clone()));
        }
        Patch::UpdateEntry { locator, entry } => {
            let index = located();
            let base = entry_path(locator.list, index);
            let current = match splice::lookup(tree, &base) {
                Some(Value::Table(current)) => current.clone(),
                _ => Table::new(),
            };
            for (field, value) in entry {
                if !splice::same(current.get(field), Some(value)) {
                    set_ops(tree, field_path(&base, field), value, &mut ops);
                }
            }
            for field in current.keys() {
                if !entry.contains_key(field) {
                    ops.push(Op::Remove(field_path(&base, field)));
                }
            }
        }
        Patch::SetEntryField {
            locator,
            field,
            value,
        } => {
            let index = located();
            let field = field_path(&entry_path(locator.list, index), field);
            set_ops(tree, field, value, &mut ops);
        }
        Patch::ClearEntryField { locator, field } => {
            let index = located();
            ops.push(Op::Remove(field_path(
                &entry_path(locator.list, index),
                field,
            )));
        }
        Patch::RemoveEntry { locator } => {
            let index = located();
            ops.push(Op::RemoveEntry(locator.list, index));
        }
    }
    ops
}

fn entry_path(list: List, index: usize) -> Vec<Seg> {
    vec![Seg::Key(list.key().into()), Seg::Index(index)]
}

fn field_path(entry: &[Seg], field: &str) -> Vec<Seg> {
    let mut path = entry.to_vec();
    path.push(Seg::Key(field.into()));
    path
}

/// A table set onto an existing table merges key by key, so the file keeps
/// the layout and comments of what stays. A new non-empty table outside an
/// entry becomes its own keys, which puts it in a `[table]` section.
fn set_ops(tree: &Table, path: Vec<Seg>, value: &Value, ops: &mut Vec<Op>) {
    let in_entry =
        path.len() >= 2 && matches!(path[path.len() - 2], Seg::Index(_));
    match (splice::lookup(tree, &path), value) {
        (Some(Value::Table(current)), Value::Table(table)) => {
            for (key, value) in table {
                if !splice::same(current.get(key), Some(value)) {
                    set_ops(tree, field_path(&path, key), value, ops);
                }
            }
            for key in current.keys() {
                if !table.contains_key(key) {
                    ops.push(Op::Remove(field_path(&path, key)));
                }
            }
        }
        (None, Value::Table(table)) if !table.is_empty() && !in_entry => {
            for (key, value) in table {
                set_ops(tree, field_path(&path, key), value, ops);
            }
        }
        _ => ops.push(Op::Set(path, value.clone())),
    }
}

enum Written {
    Replaced(Snapshot),
    /// The target changed before the rename; nothing was replaced.
    Changed,
}

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Steps 5 to 7: write a temporary file beside the target, recheck that the
/// logical path still reaches the same unchanged file, and rename over it.
/// `created` collects the directories this and earlier attempts created.
fn write(
    logical: &Path,
    target: &Target,
    candidate: &str,
    created: &mut Vec<PathBuf>,
    hooks: &dyn Hooks,
) -> Result<Written, ApplyError> {
    let directory = target
        .path
        .parent()
        .ok_or_else(|| {
            ApplyError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the configuration path has no directory",
            ))
        })?
        .to_owned();
    created.extend(create_directories(&directory).map_err(ApplyError::Io)?);
    let name = target
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let prefix = format!(".{name}.huterm-tmp-");
    remove_stale_temporaries(&directory, &prefix);
    let temporary = directory.join(format!(
        "{prefix}{}-{}",
        std::process::id(),
        TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        write_temporary(&temporary, target, candidate.as_bytes(), hooks)?;
        hooks.before(Stage::Recheck)?;
        let again = match resolve(logical) {
            Ok(again) => Some(again),
            // The link now leads nowhere: a change like any other.
            Err(ApplyError::DanglingSymlink(_)) => None,
            Err(ApplyError::Io(error)) => return Err(error),
            Err(other) => return Err(io::Error::other(other.to_string())),
        };
        if again.as_ref() != Some(target) {
            return Ok(false);
        }
        hooks.before(Stage::Rename)?;
        fs::rename(&temporary, &target.path)?;
        Ok(true)
    })();
    match result {
        Ok(true) => {}
        Ok(false) => {
            let _ = fs::remove_file(&temporary);
            return Ok(Written::Changed);
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(ApplyError::Io(error));
        }
    }
    sync_directories(&directory, created, hooks)
        .map_err(ApplyError::WrittenNotDurable)?;
    let metadata =
        fs::metadata(&target.path).map_err(ApplyError::WrittenNotDurable)?;
    let written = Target {
        path: target.path.clone(),
        identity: identity(&metadata),
        content: Some(candidate.as_bytes().to_vec()),
    };
    Ok(Written::Replaced(Snapshot {
        text: Some(candidate.to_owned()),
        revision: written.revision(),
    }))
}

fn write_temporary(
    temporary: &Path,
    target: &Target,
    bytes: &[u8],
    hooks: &dyn Hooks,
) -> io::Result<()> {
    hooks.before(Stage::TempWrite)?;
    let mut file = fs::File::options()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    if target.content.is_some() {
        let permissions = fs::metadata(&target.path)?.permissions();
        file.set_permissions(permissions)?;
    }
    file.write_all(bytes)?;
    hooks.before(Stage::TempSync)?;
    file.sync_all()
}

/// Creates the missing directories above the target, returning those this
/// call created, outermost first.
fn create_directories(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut ancestor = Some(directory);
    while let Some(path) = ancestor {
        if path.as_os_str().is_empty() || path.exists() {
            break;
        }
        missing.push(path.to_owned());
        ancestor = path.parent();
    }
    let mut created = Vec::new();
    for path in missing.into_iter().rev() {
        match fs::create_dir(&path) {
            Ok(()) => created.push(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Ok(created)
}

/// Makes the rename durable: the target's directory, then the parent of
/// each directory this call created, innermost first.
fn sync_directories(
    directory: &Path,
    created: &[PathBuf],
    hooks: &dyn Hooks,
) -> io::Result<()> {
    hooks.before(Stage::DirectorySync)?;
    sync_directory(directory)?;
    for path in created.iter().rev() {
        if let Some(parent) = path.parent() {
            hooks.before(Stage::ParentSync)?;
            sync_directory(parent)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Removes temporary files a crashed save left behind. A fresh one may
/// belong to another process's save in progress, so only old ones go.
fn remove_stale_temporaries(directory: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > STALE_TEMPORARY)
            });
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests;
