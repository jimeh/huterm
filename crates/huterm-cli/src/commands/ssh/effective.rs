//! The configuration `ssh -G` prints for a command line.
use std::collections::BTreeMap;

use crate::commands::ssh_cache::Key;

/// The `ssh -G` settings Huterm reads, by lowercase keyword.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Effective(BTreeMap<String, String>);

/// The keywords Huterm reads; `ssh -G` prints many more.
const READ: [&str; 7] = [
    "user",
    "hostname",
    "sessiontype",
    "forkafterauthentication",
    "stdinnull",
    "requesttty",
    "remotecommand",
];

impl Effective {
    /// Reads `keyword value` lines, keeping the first value of each.
    pub(crate) fn parse(text: &str) -> Self {
        let mut settings = BTreeMap::new();
        for line in text.lines() {
            let Some((keyword, value)) = line.split_once(' ') else {
                continue;
            };
            let keyword = keyword.to_ascii_lowercase();
            if READ.contains(&keyword.as_str()) {
                settings.entry(keyword).or_insert_with(|| value.to_owned());
            }
        }
        Self(settings)
    }

    fn get(&self, keyword: &str) -> Option<&str> {
        self.0.get(keyword).map(String::as_str)
    }

    /// The cache key, `user@hostname`, with `HostName` applied.
    pub(crate) fn key(&self) -> Option<Key> {
        Key::of(self.get("user")?, self.get("hostname")?).ok()
    }

    /// Whether the effective settings start an interactive session whose
    /// standard input an install can borrow first. A setting `ssh -G` does
    /// not print, as before OpenSSH 8.7, rules the install out.
    pub(crate) fn interactive(&self) -> bool {
        self.get("sessiontype") == Some("default")
            && self.get("forkafterauthentication") == Some("no")
            && self.get("stdinnull") == Some("no")
            && matches!(self.get("remotecommand"), None | Some("none"))
            && self
                .get("requesttty")
                .is_some_and(|request| request != "no")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ssh -G` output from OpenSSH 10.2 for an interactive session to a
    /// host alias whose `HostName` and `User` come from configuration,
    /// trimmed to a representative set of lines.
    const CAPTURED: &str = "\
host work
user alice
hostname build.example.org
port 2222
addressfamily any
batchmode no
controlmaster auto
controlpath /home/alice/.ssh/cm-%C
controlpersist 600
forkafterauthentication no
stdinnull no
sessiontype default
requesttty auto
proxyjump bastion.example.org
sendenv LANG
sendenv LC_*
";

    fn with(replacements: &[(&str, &str)]) -> Effective {
        let mut text = CAPTURED.to_owned();
        for (from, to) in replacements {
            assert!(text.contains(from), "{from}");
            text = text.replace(from, to);
        }
        Effective::parse(&text)
    }

    #[test]
    fn the_key_applies_host_name_and_ignores_alias_jump_and_multiplexing() {
        let settings = Effective::parse(CAPTURED);
        assert_eq!(settings.key(), Key::parse("alice@build.example.org").ok());
        assert!(
            settings.interactive(),
            "a user's ControlPath changes nothing"
        );
        assert_eq!(
            with(&[("controlpath /home/alice/.ssh/cm-%C\n", "")]),
            settings,
            "nor does its absence"
        );
        assert_eq!(with(&[("user alice\n", "")]).key(), None);
        assert_eq!(with(&[("hostname build.example.org\n", "")]).key(), None);
        assert_eq!(with(&[("user alice", "al ice")]).key(), None);
        assert_eq!(
            with(&[("user alice", "USER bob\nuser alice")]).key(),
            Key::parse("bob@build.example.org").ok(),
            "keywords are case-insensitive and the first value wins"
        );
    }

    #[test]
    fn each_noninteractive_setting_rules_out_an_install() {
        for replacement in [
            ("sessiontype default", "sessiontype none"),
            ("sessiontype default", "sessiontype subsystem"),
            ("forkafterauthentication no", "forkafterauthentication yes"),
            ("stdinnull no", "stdinnull yes"),
            ("requesttty auto", "requesttty no"),
            ("requesttty auto", "remotecommand uptime\nrequesttty auto"),
            ("sessiontype default\n", ""),
            ("forkafterauthentication no\n", ""),
            ("stdinnull no\n", ""),
            ("requesttty auto\n", ""),
        ] {
            assert!(!with(&[replacement]).interactive(), "{replacement:?}");
        }
        for replacement in [
            ("requesttty auto", "requesttty yes"),
            ("requesttty auto", "requesttty force"),
            ("requesttty auto", "remotecommand none\nrequesttty auto"),
        ] {
            assert!(with(&[replacement]).interactive(), "{replacement:?}");
        }
    }
}
