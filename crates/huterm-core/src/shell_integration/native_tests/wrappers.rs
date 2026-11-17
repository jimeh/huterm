//! The `ssh` and `sudo` wrappers in real shells, with stand-in `ssh`,
//! `sudo`, and `huterm` executables that log each call and its input. Shell
//! availability follows `HUTERM_REQUIRE_SHELLS` through [`executable`].

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
/// stand-in `huterm` in its `HUTERM_BIN_DIR`. Each stand-in runs `record`
/// with its name, which appends the name and arguments to `$HOME/log`, each
/// followed by a NUL, then a newline, and writes its standard input, or
/// `<terminal>` when that is a terminal, to `$HOME/stdin`. User functions
/// in the startup files call `record` too.
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
        let record = home.0.join("tools/record");
        let script = |tool: &str| {
            if tool == "record" {
                format!(
                    "#!/bin/sh\n\
                     if [ -t 0 ]; then printf '<terminal>' > '{stdin}'; \
                     else cat > '{stdin}'; fi\n\
                     printf '%s\\0' \"$@\" >> '{log}'\n\
                     printf '\\n' >> '{log}'\n",
                    stdin = home.0.join("stdin").display(),
                    log = home.0.join("log").display(),
                )
            } else {
                format!(
                    "#!/bin/sh\nexec '{}' {tool} \"$@\"\n",
                    record.display()
                )
            }
        };
        for (directory, tool) in [
            ("tools", "record"),
            ("tools", "ssh"),
            ("tools", "sudo"),
            ("bin", "huterm"),
        ] {
            if tool == "huterm" && !huterm {
                fs::create_dir_all(home.0.join(directory)).unwrap();
                continue;
            }
            let file = format!("{directory}/{tool}");
            home.write(&file, &script(tool));
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

    /// Runs `input` and returns the calls it logged, each as its
    /// arguments joined by spaces, with an argument that holds whitespace or
    /// is empty in single quotes.
    fn run(&mut self, input: &str) -> Vec<String> {
        self.shell.plain(input);
        let log =
            fs::read_to_string(self.home.0.join("log")).unwrap_or_default();
        let lines: Vec<String> = log
            .lines()
            .map(|line| {
                line.strip_suffix('\0')
                    .unwrap_or(line)
                    .split('\0')
                    .map(|argument| {
                        if argument.is_empty()
                            || argument.contains(char::is_whitespace)
                        {
                            format!("'{argument}'")
                        } else {
                            argument.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let added = lines[self.seen..].to_vec();
        self.seen = lines.len();
        added
    }

    /// The standard input the last call received.
    fn stdin(&self) -> String {
        fs::read_to_string(self.home.0.join("stdin")).unwrap()
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
        shell.expect(
            "ssh a 'echo b  c'",
            "huterm ssh --forward-env --terminfo a 'echo b  c'",
        );
        shell.expect("sudo -u root 'x  y'", "huterm sudo -u root 'x  y'");
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
        shell.expect("ssh a 'echo b  c'", "ssh a 'echo b  c'");
        shell.expect("sudo x", "sudo x");
        eprintln!("native {name}: wrapper feature combinations ran");
    }
}

#[test]
fn native_wrappers_forward_piped_input_and_keep_the_terminal() {
    for (name, binary) in shells() {
        let pipe = if name == "nu" {
            "'piped data' |"
        } else {
            "printf 'piped data' |"
        };
        for (huterm, ssh, sudo) in [
            (
                true,
                "huterm ssh --forward-env --terminfo a 'echo b  c'",
                "huterm sudo tee f",
            ),
            (false, "ssh a 'echo b  c'", "sudo tee f"),
        ] {
            let mut shell = Wrapped::spawn(name, &binary, &ALL, huterm, "");
            for (input, expected) in
                [("ssh a 'echo b  c'", ssh), ("sudo tee f", sudo)]
            {
                shell.expect(input, expected);
                assert_eq!(shell.stdin(), "<terminal>", "{name}: {input}");
                shell.expect(&format!("{pipe} {input}"), expected);
                assert_eq!(shell.stdin(), "piped data", "{name}: {input}");
            }
        }
        eprintln!("native {name}: wrappers forward piped input");
    }
}

#[test]
fn native_wrappers_respect_user_functions_aliases_and_later_changes() {
    for (name, binary) in shells() {
        if name == "nu" {
            // Huterm imports a wrapper after config.nu only when its
            // features are on; then it replaces the user's, as documented.
            use ShellFeature::{Path as PathFeature, SshEnv, Sudo};
            let startup = "def --wrapped ssh [...rest] { ^record user-ssh ...$rest }\n\
                           def --wrapped sudo [...rest] { ^record user-sudo ...$rest }";
            for (features, ssh, sudo) in [
                (
                    &ALL[..],
                    "huterm ssh --forward-env --terminfo a",
                    "huterm sudo x",
                ),
                (&[PathFeature, Sudo], "user-ssh a", "huterm sudo x"),
                (
                    &[PathFeature, SshEnv],
                    "huterm ssh --forward-env a",
                    "user-sudo x",
                ),
                (&[PathFeature], "user-ssh a", "user-sudo x"),
            ] {
                let mut shell =
                    Wrapped::spawn(name, &binary, features, true, startup);
                shell.expect("ssh a", ssh);
                shell.expect("sudo x", sudo);
            }
            eprintln!("native nu: wrappers replace user defs only when on");
            continue;
        }
        let (startup, user_sudo) = if name == "fish" {
            (
                "alias ssh='ssh -A'\nfunction sudo; record user-sudo $argv; end"
                    .to_owned(),
                "function sudo; record user-sudo $argv; end".to_owned(),
            )
        } else {
            let sudo = "function sudo { record user-sudo \"$@\"; }";
            (format!("alias ssh='ssh -A'\n{sudo}"), sudo.to_owned())
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
