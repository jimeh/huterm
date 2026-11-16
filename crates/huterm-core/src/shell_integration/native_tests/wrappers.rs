//! The `ssh` and `sudo` wrappers in real shells, with stand-in `ssh`,
//! `sudo`, and `huterm` executables that log each call. Shell availability
//! follows `HUTERM_REQUIRE_SHELLS` through [`executable`].

use std::os::unix::fs::PermissionsExt as _;

use huterm_protocol::ShellFeature;

use super::*;

const ALL: [ShellFeature; 4] = [
    ShellFeature::Path,
    ShellFeature::SshEnv,
    ShellFeature::SshTerminfo,
    ShellFeature::Sudo,
];

/// A shell with stand-ins on `PATH` and, unless `huterm` is false, a
/// stand-in `huterm` in its `HUTERM_BIN_DIR`. Each stand-in appends its
/// name and arguments to `$HOME/log`.
struct Wrapped {
    shell: Shell,
    home: Directory,
    name: &'static str,
    seen: usize,
}

impl Wrapped {
    fn spawn(
        name: &'static str,
        binary: &Path,
        features: &[ShellFeature],
        huterm: bool,
        startup: &str,
    ) -> Self {
        let (home, mut command) = fixture(name, false);
        binary.clone_into(&mut command.program);
        command.shell_integration.features = features.iter().copied().collect();
        let log = home.0.join("log");
        for (directory, tool) in
            [("tools", "ssh"), ("tools", "sudo"), ("bin", "huterm")]
        {
            if tool == "huterm" && !huterm {
                fs::create_dir_all(home.0.join(directory)).unwrap();
                continue;
            }
            let file = format!("{directory}/{tool}");
            home.write(
                &file,
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"{tool} $*\" >> '{}'\n",
                    log.display()
                ),
            );
            fs::set_permissions(
                home.0.join(&file),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        command.environment.push((
            "PATH".into(),
            format!(
                "{}:{}",
                home.0.join("tools").display(),
                std::env::var("PATH").unwrap()
            ),
        ));
        if !startup.is_empty() {
            let rc = match name {
                "bash" | "/bin/bash" => ".bashrc",
                "zsh" => ".zshrc",
                "fish" => ".config/fish/config.fish",
                "nu" => ".config/nushell/config.nu",
                _ => unreachable!(),
            };
            let existing = fs::read_to_string(home.0.join(rc)).unwrap();
            home.write(rc, &format!("{existing}{startup}\n"));
        }
        let process = crate::pty::spawn_with_plan_isolated(
            &command,
            launch_plan_in(&command, None, home.0.join("bin")),
        )
        .unwrap();
        let mut shell = Shell::from_process(&command, process);
        shell.prompt();
        Self {
            shell,
            home,
            name,
            seen: 0,
        }
    }

    /// Runs `input` and returns the log lines it added.
    fn run(&mut self, input: &str) -> Vec<String> {
        self.shell.plain(input);
        let log =
            fs::read_to_string(self.home.0.join("log")).unwrap_or_default();
        let lines: Vec<String> = log.lines().map(str::to_owned).collect();
        let added = lines[self.seen..].to_vec();
        self.seen = lines.len();
        added
    }

    /// Asserts that `input` logged exactly `expected`.
    fn expect(&mut self, input: &str, expected: &str) {
        let added = self.run(input);
        assert_eq!(
            added,
            [expected],
            "{}: {input}: {:?}",
            self.name,
            String::from_utf8_lossy(&self.shell.transcript)
        );
    }

    /// The bypass that runs the program itself.
    fn bypass(&self, program: &str) -> String {
        if self.name == "nu" {
            format!("^{program}")
        } else {
            format!("command {program}")
        }
    }
}

/// The shells with automatic wrappers: Apple's Bash 3.2 too on macOS.
fn shells() -> Vec<(&'static str, PathBuf)> {
    let mut names = vec!["bash", "zsh", "fish", "nu"];
    if cfg!(target_os = "macos") {
        names.push("/bin/bash");
    }
    names
        .into_iter()
        .filter_map(|name| executable(name).map(|binary| (name, binary)))
        .collect()
}

#[test]
fn native_wrappers_pass_flags_per_feature_and_fall_back() {
    use ShellFeature::{Path as PathFeature, SshEnv, SshTerminfo, Sudo};
    for (name, binary) in shells() {
        // Every feature: available at the first prompt, and bypassable.
        let mut shell = Wrapped::spawn(name, &binary, &ALL, true, "");
        shell
            .expect("ssh a 'b c'", "huterm ssh --forward-env --terminfo a b c");
        shell.expect("sudo -u root x", "huterm sudo -u root x");
        let bypass = shell.bypass("ssh");
        shell.expect(&format!("{bypass} c"), "ssh c");
        let bypass = shell.bypass("sudo");
        shell.expect(&format!("{bypass} d"), "sudo d");

        for (features, ssh, sudo) in [
            (
                &[PathFeature, SshEnv][..],
                "huterm ssh --forward-env a",
                "sudo x",
            ),
            (
                &[SshTerminfo, Sudo],
                "huterm ssh --terminfo a",
                "huterm sudo x",
            ),
            (&[PathFeature], "ssh a", "sudo x"),
        ] {
            let mut shell = Wrapped::spawn(name, &binary, features, true, "");
            shell.expect("ssh a", ssh);
            shell.expect("sudo x", sudo);
        }

        // Without the huterm command, the wrappers run the programs.
        let mut shell = Wrapped::spawn(name, &binary, &ALL, false, "");
        shell.expect("ssh a", "ssh a");
        shell.expect("sudo x", "sudo x");
        eprintln!("native {name}: wrapper feature combinations ran");
    }
}

#[test]
fn native_wrappers_respect_user_functions_aliases_and_later_changes() {
    for (name, binary) in shells() {
        let log = "\"$HOME/log\"";
        if name == "nu" {
            // `use huterm *` runs after config.nu, so Huterm's definitions
            // replace the user's, as documented.
            let startup = "def ssh [...rest] { $\"user-ssh ($rest | str join ' ')\\n\" | save --append ($env.HOME | path join log) }";
            let mut shell = Wrapped::spawn(name, &binary, &ALL, true, startup);
            shell.expect("ssh a", "huterm ssh --forward-env --terminfo a");
            eprintln!("native nu: Huterm's wrapper replaces a user def");
            continue;
        }
        let (startup, user_sudo) = if name == "fish" {
            (
                "alias ssh='ssh -A'\nfunction sudo; echo user-sudo $argv >> $HOME/log; end".to_owned(),
                "function sudo; echo user-sudo $argv >> $HOME/log; end".to_owned(),
            )
        } else {
            let sudo = format!(
                "function sudo {{ printf 'user-sudo %s\\n' \"$*\" >> {log}; }}"
            );
            (format!("alias ssh='ssh -A'\n{sudo}"), sudo)
        };
        let mut shell = Wrapped::spawn(name, &binary, &ALL, true, &startup);
        // A fish alias is a function, so the wrapper stays out of its way.
        let aliased = if name == "fish" {
            "ssh -A a"
        } else {
            "huterm ssh --forward-env --terminfo -A a"
        };
        shell.expect("ssh a", aliased);
        shell.expect("sudo x", "user-sudo x");

        let mut shell = Wrapped::spawn(name, &binary, &ALL, true, "");
        shell.expect("ssh a", "huterm ssh --forward-env --terminfo a");
        let remove = match name {
            "zsh" => "unfunction ssh",
            "fish" => "functions -e ssh",
            _ => "unset -f ssh",
        };
        shell.run(remove);
        shell.expect("ssh b", "ssh b");
        shell.expect("ssh c", "ssh c");
        shell.run(&user_sudo);
        shell.expect("sudo x", "user-sudo x");
        shell.expect("sudo y", "user-sudo y");
        eprintln!(
            "native {name}: user functions, aliases, and later changes ran"
        );
    }
}
