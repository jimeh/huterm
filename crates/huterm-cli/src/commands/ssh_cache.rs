//! `huterm ssh-cache`: the record of SSH destinations known to have the
//! `xterm-huterm` terminfo entry, which `huterm ssh` reads and writes.
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::Subcommand;

use super::{Kind, check_owned};

/// The cache's file name under the state root.
const FILE_NAME: &str = "ssh-cache";

/// The most bytes a read takes from the cache file.
const READ_LIMIT: u64 = 1 << 20;

#[derive(Debug, Subcommand)]
pub(crate) enum Action {
    /// Print each host and when the entry was installed, in UTC
    List,
    /// Add hosts, or refresh their install time
    Add {
        /// Hosts as user@host
        #[arg(required = true, value_name = "USER@HOST", value_parser = Key::parse)]
        keys: Vec<Key>,
    },
    /// Remove hosts; one that is absent fails after the rest are removed
    Remove {
        /// Hosts as user@host
        #[arg(required = true, value_name = "USER@HOST", value_parser = Key::parse)]
        keys: Vec<Key>,
    },
    /// Remove hosts installed longer ago than AGE
    Prune {
        /// An integer followed by s, m, h, d, or w, such as 30d
        #[arg(value_name = "AGE", value_parser = parse_age)]
        age: u64,
    },
    /// Remove every host
    Clear,
}

/// A destination as `user@host`: split at the last `@`, both parts
/// non-empty, without whitespace or control characters.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Key(String);

impl Key {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let valid = value
            .rsplit_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
            && !value.chars().any(|character| {
                character.is_whitespace() || character.is_control()
            });
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(format!(
                "{value:?} is not user@host without spaces or control characters"
            ))
        }
    }

    /// The key for `user` at `host`.
    pub(crate) fn of(user: &str, host: &str) -> Result<Self, String> {
        Self::parse(&format!("{user}@{host}"))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Each key and its install time, in seconds since the Unix epoch.
pub(crate) type Entries = BTreeMap<Key, u64>;

/// The cache file and the directory that holds it.
#[derive(Debug)]
pub(crate) struct Store {
    directory: PathBuf,
    uid: u32,
}

impl Store {
    /// The cache under Huterm's state root.
    pub(crate) fn locate() -> anyhow::Result<Self> {
        let root = huterm_state::state_root(|name| std::env::var_os(name))
            .context("cannot locate the SSH cache")?;
        Ok(Self::at(root, huterm_socket::effective_uid()))
    }

    fn at(directory: PathBuf, uid: u32) -> Self {
        Self { directory, uid }
    }

    fn file(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    /// Reads the entries, skipping malformed lines. A missing directory or
    /// file is an empty cache.
    pub(crate) fn load(&self) -> anyhow::Result<Entries> {
        match std::fs::symlink_metadata(&self.directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Entries::new());
            }
            _ => {}
        }
        self.check_directory()?;
        let file = self.file();
        if !self.check_file()? {
            return Ok(Entries::new());
        }
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(&file)
            .and_then(|handle| handle.take(READ_LIMIT).read_to_end(&mut bytes))
            .with_context(|| format!("cannot read {}", file.display()))?;
        Ok(parse(&String::from_utf8_lossy(&bytes)))
    }

    /// Replaces the file with `entries`: writes a new private file beside
    /// it, then renames it over the old one.
    pub(crate) fn save(&self, entries: &Entries) -> anyhow::Result<()> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)
            .with_context(|| {
                format!("cannot create {}", self.directory.display())
            })?;
        self.check_directory()?;
        self.check_file()?;
        let temporary = self.directory.join(format!(
            ".{FILE_NAME}.{}.{:016x}.tmp",
            std::process::id(),
            super::random()
        ));
        let write = || -> std::io::Result<()> {
            let mut handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            handle.write_all(format(entries).as_bytes())?;
            handle.sync_all()?;
            std::fs::rename(&temporary, self.file())
        };
        write().map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            anyhow::Error::new(error)
                .context(format!("cannot write {}", self.file().display()))
        })
    }

    fn check_directory(&self) -> anyhow::Result<()> {
        check_owned(&self.directory, Kind::Directory, self.uid)
            .map_err(anyhow::Error::msg)
    }

    /// Checks the file when it exists, and returns whether it does.
    fn check_file(&self) -> anyhow::Result<bool> {
        let file = self.file();
        match std::fs::symlink_metadata(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(false)
            }
            _ => check_owned(&file, Kind::File, self.uid)
                .map(|()| true)
                .map_err(anyhow::Error::msg),
        }
    }
}

/// Parses `user@host<TAB><unix seconds>` lines, skipping any other line.
/// A repeated key keeps its latest time.
fn parse(text: &str) -> Entries {
    let mut entries = Entries::new();
    for line in text.lines() {
        let Some((key, time)) = line.split_once('\t') else {
            continue;
        };
        let (Ok(key), Ok(time)) = (Key::parse(key), time.parse::<u64>()) else {
            continue;
        };
        let entry = entries.entry(key).or_insert(time);
        *entry = (*entry).max(time);
    }
    entries
}

fn format(entries: &Entries) -> String {
    let mut text = String::new();
    for (key, time) in entries {
        let _ = writeln!(text, "{key}\t{time}");
    }
    text
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Parses an age such as `30d` into seconds.
fn parse_age(value: &str) -> Result<u64, String> {
    let invalid =
        || format!("{value:?} is not an integer followed by s, m, h, d, or w");
    let unit = value.chars().last().ok_or_else(invalid)?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 60 * 60,
        'd' => 24 * 60 * 60,
        'w' => 7 * 24 * 60 * 60,
        _ => return Err(invalid()),
    };
    let digits = &value[..value.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    digits
        .parse::<u64>()
        .ok()
        .and_then(|count| count.checked_mul(scale))
        .ok_or_else(invalid)
}

/// Formats seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    // Howard Hinnant's days-to-civil conversion, for days since 1970-01-01.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524
        - day_of_era / 146_096)
        / 365;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3600,
        remainder / 60 % 60,
        remainder % 60
    )
}

pub(crate) fn run(action: Action) -> anyhow::Result<()> {
    let store = Store::locate()?;
    apply(&store, action, now(), &mut std::io::stdout().lock())
}

fn apply(
    store: &Store,
    action: Action,
    now: u64,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let mut entries = store.load()?;
    let mut absent = Vec::new();
    match action {
        Action::List => {
            for (key, time) in &entries {
                writeln!(out, "{key}\t{}", utc(*time))?;
            }
            return Ok(());
        }
        Action::Add { keys } => {
            for key in keys {
                entries.insert(key, now);
            }
        }
        Action::Remove { keys } => {
            for key in keys {
                if entries.remove(&key).is_none() {
                    absent.push(key);
                }
            }
        }
        Action::Prune { age } => {
            entries.retain(|_, time| now.saturating_sub(*time) <= age);
        }
        Action::Clear => entries.clear(),
    }
    store.save(&entries)?;
    let Some(last) = absent.pop() else {
        return Ok(());
    };
    for key in absent {
        eprintln!("huterm: {key} is not in the SSH cache");
    }
    anyhow::bail!("{last} is not in the SSH cache")
}

#[cfg(test)]
mod tests;
