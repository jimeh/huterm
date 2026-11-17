use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use super::*;

/// A directory removed on drop, unique per test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("huterm-cli-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn store(&self) -> Store {
        Store::at(self.0.join("state"), huterm_socket::effective_uid())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(value: &str) -> Key {
    Key::parse(value).unwrap()
}

fn listed(store: &Store) -> String {
    let mut out = Vec::new();
    apply(store, Action::List, 0, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn keys_need_a_user_and_host_without_spaces_or_controls() {
    for valid in ["alice@example.org", "a@b", "me@work@bastion", "root@::1"] {
        assert!(Key::parse(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "",
        "example.org",
        "@host",
        "user@",
        "a b@host",
        "user@ho\tst",
        "user@host\n",
        "user@\u{7f}",
        "user@\u{85}",
    ] {
        assert!(Key::parse(invalid).is_err(), "{invalid:?}");
    }
    assert_eq!(
        Key::of("alice", "example.org").unwrap(),
        key("alice@example.org")
    );
}

#[test]
fn ages_take_an_integer_and_one_unit() {
    for (value, seconds) in [
        ("0s", 0),
        ("90s", 90),
        ("2m", 120),
        ("3h", 10_800),
        ("1d", 86_400),
        ("2w", 1_209_600),
    ] {
        assert_eq!(parse_age(value), Ok(seconds), "{value}");
    }
    for value in [
        "",
        "d",
        "1",
        "1y",
        "-1d",
        "1.5d",
        " 1d",
        "1d ",
        "99999999999999999999w",
    ] {
        assert!(parse_age(value).is_err(), "{value:?}");
    }
}

#[test]
fn install_times_print_as_utc() {
    for (seconds, text) in [
        (0, "1970-01-01T00:00:00Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_798_761_599, "2026-12-31T23:59:59Z"),
        (4_107_542_400, "2100-03-01T00:00:00Z"),
    ] {
        assert_eq!(utc(seconds), text, "{seconds}");
    }
}

#[test]
fn malformed_lines_are_skipped_and_repeats_keep_the_latest_time() {
    let entries = parse(
        "a@h\t10\nnot a line\nb@h\tsoon\n@h\t5\nc@h 7\na@h\t30\na@h\t20\n\nd@h\t1\textra\n",
    );
    assert_eq!(entries, Entries::from([(key("a@h"), 30)]));
}

#[test]
fn operations_add_refresh_remove_prune_and_clear() {
    let scratch = Scratch::new("operations");
    let store = scratch.store();
    assert_eq!(listed(&store), "", "a missing cache is empty");
    assert!(!store.directory.exists(), "listing creates nothing");

    let add = |keys: &[&str], now| {
        apply(
            &store,
            Action::Add {
                keys: keys.iter().copied().map(key).collect(),
            },
            now,
            &mut Vec::new(),
        )
    };
    add(&["b@h", "a@h"], 100).unwrap();
    assert_eq!(mode(&store.directory), 0o700);
    assert_eq!(mode(&store.file()), 0o600);
    assert_eq!(
        listed(&store),
        "a@h\t1970-01-01T00:01:40Z\nb@h\t1970-01-01T00:01:40Z\n"
    );
    add(&["a@h"], 200).unwrap();
    assert_eq!(
        std::fs::read_to_string(store.file()).unwrap(),
        "a@h\t200\nb@h\t100\n",
        "adding refreshes the time"
    );

    let error = apply(
        &store,
        Action::Remove {
            keys: vec![key("a@h"), key("x@h")],
        },
        300,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "x@h is not in the SSH cache");
    assert_eq!(
        std::fs::read_to_string(store.file()).unwrap(),
        "b@h\t100\n",
        "an absent key fails after removing the rest"
    );

    add(&["c@h"], 1000).unwrap();
    apply(&store, Action::Prune { age: 899 }, 1000, &mut Vec::new()).unwrap();
    assert_eq!(
        std::fs::read_to_string(store.file()).unwrap(),
        "c@h\t1000\n"
    );
    apply(&store, Action::Prune { age: 0 }, 1000, &mut Vec::new()).unwrap();
    assert_eq!(
        std::fs::read_to_string(store.file()).unwrap(),
        "c@h\t1000\n",
        "only entries older than the age go"
    );

    apply(&store, Action::Clear, 1001, &mut Vec::new()).unwrap();
    assert_eq!(listed(&store), "");
    let leftovers: Vec<_> = std::fs::read_dir(&store.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["ssh-cache"], "writes leave no temporary file");
}

#[test]
fn reads_stop_at_the_size_limit() {
    let scratch = Scratch::new("limit");
    let store = scratch.store();
    std::fs::create_dir_all(&store.directory).unwrap();
    let mut text = "a@h\t1\n".to_owned();
    text.push_str(&"#".repeat(usize::try_from(READ_LIMIT).unwrap()));
    text.push_str("\nb@h\t2\n");
    std::fs::write(store.file(), text).unwrap();
    assert_eq!(store.load().unwrap(), Entries::from([(key("a@h"), 1)]));
}

#[test]
fn links_and_other_owners_fail_every_operation() {
    let scratch = Scratch::new("unsafe");
    let real = scratch.0.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("ssh-cache"), "a@h\t1\n").unwrap();

    // A linked directory.
    symlink(&real, scratch.0.join("state")).unwrap();
    let store = scratch.store();
    for action in [Action::List, Action::Clear] {
        let error = apply(&store, action, 0, &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }

    // A linked file.
    std::fs::remove_file(scratch.0.join("state")).unwrap();
    std::fs::create_dir(scratch.0.join("state")).unwrap();
    symlink(real.join("ssh-cache"), store.file()).unwrap();
    for action in [Action::List, Action::Clear] {
        let error = apply(&store, action, 0, &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }
    assert_eq!(
        std::fs::read_to_string(real.join("ssh-cache")).unwrap(),
        "a@h\t1\n",
        "the link's target is untouched"
    );

    // Another owner, simulated with a different expected user.
    std::fs::remove_file(store.file()).unwrap();
    let foreign =
        Store::at(store.directory.clone(), huterm_socket::effective_uid() + 1);
    let error = apply(&foreign, Action::List, 0, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("owns it"), "{error}");
}
