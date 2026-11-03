//! Real shells through production PTY spawning. No model, UI, or emulator mocks.
use super::*;
use std::fs;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "huterm-shell-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, text: &str) {
        let file = self.0.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Shell {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    waiter: crate::pty::ReadinessWaiter,
    // Drop taps before the process, so its cleanup closes the last PTY handles.
    _process: crate::pty::PtyProcess,
    transcript: Vec<u8>,
    engine: crate::engine::TerminalEngine,
    directory: Option<huterm_protocol::TerminalDirectory>,
}
impl Shell {
    fn spawn(command: &TerminalCommand) -> Self {
        let process = crate::pty::spawn_with_plan_isolated(
            command,
            launch_plan(command, None),
        )
        .unwrap();
        Self::from_process(command, process)
    }
    fn from_process(
        command: &TerminalCommand,
        mut process: crate::pty::PtyProcess,
    ) -> Self {
        let (reader, writer, waiter) = process.raw_tap();
        Self {
            reader,
            writer,
            waiter,
            _process: process,
            transcript: Vec::new(),
            directory: None,
            engine: crate::engine::TerminalEngine::new(
                huterm_protocol::TerminalId::new(1),
                command.grid_size,
                command.cell_size,
                command.presentation.clone(),
            )
            .unwrap(),
        }
    }
    fn send(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).unwrap();
    }
    fn until(&mut self, condition: impl Fn(&[u8]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buffer = [0; 16384];
        while !condition(&self.transcript) {
            assert!(
                Instant::now() < deadline,
                "shell timeout: {:?}",
                String::from_utf8_lossy(&self.transcript)
            );
            match self.reader.read(&mut buffer) {
                Ok(0) => panic!(
                    "shell exited: {:?}",
                    String::from_utf8_lossy(&self.transcript)
                ),
                Ok(count) => {
                    let chunk = &buffer[..count];
                    self.transcript.extend_from_slice(chunk);
                    for effect in self.engine.process(chunk).unwrap() {
                        match effect {
                            crate::engine::EngineEffect::PtyWrite(reply) => {
                                self.writer.write_all(&reply).unwrap();
                            }
                            crate::engine::EngineEffect::Directory(
                                directory,
                            ) => self.directory = directory,
                            _ => {}
                        }
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    self.waiter
                        .wait(Some(
                            deadline.saturating_duration_since(Instant::now()),
                        ))
                        .unwrap();
                }
                Err(error) => panic!(
                    "read: {error}: {:?}",
                    String::from_utf8_lossy(&self.transcript)
                ),
            }
        }
    }
    fn prompt(&mut self) {
        self.until(|bytes| String::from_utf8_lossy(bytes).contains("READY>"));
    }
    fn cycle(&mut self, command: &str) -> Vec<u8> {
        self.transcript.clear();
        self.send(&format!("{command}\n"));
        self.until(|bytes| markers(bytes).iter().any(|mark| mark == "C"));
        self.until(|bytes| {
            let marks = markers(bytes);
            marks
                .iter()
                .position(|mark| mark == "C")
                .is_some_and(|index| {
                    marks[index..].iter().any(|mark| mark == "A")
                })
        });
        self.until(|bytes| {
            let marks = markers(bytes);
            marks
                .iter()
                .position(|mark| mark == "C")
                .is_some_and(|index| {
                    marks[index..].iter().any(|mark| mark == "A")
                })
                && String::from_utf8_lossy(bytes)
                    .rsplit("]133;A")
                    .next()
                    .unwrap()
                    .contains("READY>")
        });
        self.transcript.clone()
    }
    fn plain(&mut self, command: &str) {
        self.transcript.clear();
        self.send(&format!("{command}; /usr/bin/printf '\\nHUTERM_ACK\\n'\n"));
        self.until(|bytes| {
            String::from_utf8_lossy(bytes).contains("\r\nHUTERM_ACK\r\n")
        });
        self.until(|bytes| {
            String::from_utf8_lossy(bytes)
                .rsplit("\r\nHUTERM_ACK\r\n")
                .next()
                .unwrap()
                .contains("READY>")
        });
    }
    fn capture_env(&mut self, name: &str, directory: &Directory) -> String {
        let command = format!(
            "/usr/bin/env {} '{}'",
            if name == "nu" { "out>" } else { ">" },
            directory.0.join("environment").display()
        );
        self.plain(&command);
        environment_diagnostics(
            &fs::read_to_string(directory.0.join("environment")).unwrap(),
        )
    }
}

fn environment_diagnostics(environment: &str) -> String {
    environment
        .lines()
        .filter(|line| {
            line.split_once('=').is_some_and(|(name, _)| {
                name.starts_with("HUTERM_")
                    || matches!(
                        name,
                        "PATH"
                            | "ZDOTDIR"
                            | "ENV"
                            | "HISTFILE"
                            | "XDG_DATA_DIRS"
                            | "HOME"
                    )
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn launch_plan(command: &TerminalCommand, root: Option<PathBuf>) -> LaunchPlan {
    plan(
        command,
        &Host {
            resources: root.or_else(|| {
                resources(
                    &std::env::current_exe().unwrap(),
                    cfg!(target_os = "macos"),
                )
            }),
            bin: Some(
                std::env::current_exe().unwrap().parent().unwrap().into(),
            ),
            macos: cfg!(target_os = "macos"),
            environment: &|key| {
                command
                    .environment
                    .iter()
                    .rev()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| OsString::from(value))
            },
            resolve: &resolve,
            is_file: &Path::is_file,
        },
    )
}

fn executable(name: &str) -> Option<PathBuf> {
    let mut result = resolve(
        Path::new(name),
        &std::env::current_dir().unwrap(),
        std::env::var_os("PATH").as_deref(),
    );
    if name == "bash"
        && let Some(binary) = &result
    {
        let version = std::process::Command::new(binary)
            .env_clear()
            .arg("--version")
            .output()
            .unwrap();
        if !String::from_utf8_lossy(&version.stdout).contains("version 5.") {
            result = None;
            eprintln!("shell integration: Bash 5 is unavailable");
        }
    }

    if result.is_none() {
        assert!(
            std::env::var_os("HUTERM_REQUIRE_SHELLS").is_none(),
            "required shell missing: {name}"
        );
        eprintln!("skipping shell integration: {name} missing");
    }
    result
}

fn fixture(shell: &str, login: bool) -> (Directory, TerminalCommand) {
    let directory = Directory::new();
    let home = directory.0.to_string_lossy();
    let mut command = super::tests::command(shell);
    command.working_directory.clone_from(&directory.0);
    command.shell_integration.features =
        [ShellFeature::Path].into_iter().collect();
    command.environment = vec![
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("HOME".into(), home.to_string()),
        ("ZDOTDIR".into(), home.to_string()),
        ("XDG_DATA_DIRS".into(), String::new()),
        ("XDG_CONFIG_HOME".into(), format!("{home}/.config")),
        ("TERM".into(), "xterm-256color".into()),
        ("PS1".into(), "READY> ".into()),
        ("ENV".into(), String::new()),
        ("HISTFILE".into(), String::new()),
    ];
    if login {
        command.arguments.push("-l".into());
    }
    match shell {
        "bash" | "/bin/bash" => {
            directory.write(".bashrc", "printf 'bashrc\\n' >> \"$HOME/order\"\nPS1='READY> '\n");
            directory.write(".bash_login", "echo bash_login >> \"$HOME/order\"\n");
            directory.write(".profile", "echo posix_profile >> \"$HOME/order\"\n");
            // System login profiles may set XDG paths (for example Nix).
            // The fixture's own profile controls the value asserted below.
            directory.write(".bash_profile", "printf 'profile\\n' >> \"$HOME/order\"\nPS1='READY> '\nexport XDG_DATA_DIRS=''\n");
        }
        "zsh" => {
            for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
                directory.write(name, &format!("print -r -- {name} >> \"$HOME/order\"\nPROMPT='READY> '\n"));
            }
        }
        "fish" => directory.write(".config/fish/config.fish", "echo fish >> $HOME/order\nfunction fish_prompt; printf 'READY> '; end\n"),
        "nu" => directory.write(".config/nushell/config.nu", "'nu' | save --append ($env.HOME | path join order)\n$env.config.show_banner = false\n$env.PROMPT_COMMAND = {|| 'READY> ' }\n$env.PROMPT_COMMAND_RIGHT = {|| '' }\n$env.PROMPT_INDICATOR = ''\n"),
        "elvish" => directory.write(".config/elvish/rc.elv", "echo elvish >> $E:HOME/order\ntry { use huterm-integration } catch { }\nset edit:prompt = { put 'READY> ' }\nset edit:rprompt = { }\n"),
        _ => unreachable!(),
    }
    (directory, command)
}

fn markers(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    text.split("\x1b]133;")
        .skip(1)
        .map(|part| part.split(['\x07', '\x1b']).next().unwrap().to_owned())
        .filter(|mark| !mark.starts_with("P;"))
        .map(|mark| {
            if mark.starts_with("A;") {
                "A".into()
            } else if mark.starts_with("C;") {
                "C".into()
            } else {
                mark
            }
        })
        .collect()
}

fn directory_reports(bytes: &[u8]) -> Vec<&str> {
    std::str::from_utf8(bytes)
        .unwrap()
        .split("\x1b]7;")
        .skip(1)
        .map(|part| part.split(['\x07', '\x1b']).next().unwrap())
        .collect()
}

fn run_shell(name: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    for login in [false, true] {
        let (home, mut command) = fixture(name, login);
        command.program.clone_from(&binary);
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert!(
            !String::from_utf8_lossy(&shell.transcript).contains("Error:"),
            "{name}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        assert!(
            !String::from_utf8_lossy(&shell.transcript)
                .contains("failed to load"),
            "{name}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        assert!(String::from_utf8_lossy(&shell.transcript).contains("]7;"));
        if name == "nu" {
            let reports = directory_reports(&shell.transcript);
            assert_eq!(reports.len(), 1, "one initial Nushell OSC 7 stream");
            assert!(reports[0].starts_with("kitty-shell-cwd://"));
        }
        let order = fs::read_to_string(home.0.join("order")).unwrap();
        match name {
            "bash" | "/bin/bash" => {
                assert_eq!(order, if login { "profile\n" } else { "bashrc\n" });
            }
            "zsh" => assert_eq!(
                order,
                if login {
                    ".zshenv\n.zprofile\n.zshrc\n.zlogin\n"
                } else {
                    ".zshenv\n.zshrc\n"
                }
            ),
            _ => assert!(order.contains(name)),
        }
        let env = shell.capture_env(name, &home);
        assert_environment(name, &env, true);
        let resources = resources(
            &std::env::current_exe().unwrap(),
            cfg!(target_os = "macos"),
        )
        .unwrap();
        let body = resources.join("shell-integration").join(match name {
            "bash" | "/bin/bash" => "bash/huterm.bash",
            "zsh" => "zsh/huterm.zsh",
            "fish" => "fish/huterm.fish",
            "nu" => "nushell/huterm.nu",
            "elvish" => "elvish/huterm.elv",
            _ => unreachable!(),
        });
        let source = if name == "elvish" {
            "use huterm-integration".into()
        } else {
            format!("source '{}'", body.display())
        };
        assert_stream(&mut shell, name, &source, "0");
        for (command, status) in [("true", "0"), ("false", "1")] {
            let input = if name == "nu" {
                format!("^{command}")
            } else {
                command.into()
            };
            assert_stream(&mut shell, name, &input, status);
        }
        eprintln!(
            "native {name}: startup and zero/nonzero marker cases ran (login={login})"
        );
    }
}

#[test]
fn native_zsh() {
    run_shell("zsh");
}
#[test]
fn native_bash() {
    run_shell("bash");
}
#[cfg(target_os = "macos")]
#[test]
fn native_apple_bash() {
    run_shell("/bin/bash");
}
#[test]
fn native_fish() {
    run_shell("fish");
}
#[test]
fn native_nushell() {
    run_shell("nu");
}

#[test]
fn native_nushell_respects_disabled_osc7() {
    let Some(binary) = executable("nu") else {
        return;
    };
    let (home, mut command) = fixture("nu", false);
    command.program = binary;
    let config = home.0.join(".config/nushell/config.nu");
    let mut startup = fs::read_to_string(&config).unwrap();
    startup.push_str("$env.config.shell_integration.osc7 = false\n");
    fs::write(config, startup).unwrap();
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    assert_eq!(directory_reports(&shell.transcript), [] as [&str; 0]);
    shell.cycle("^false");
    shell.until(|bytes| markers(bytes).last().is_some_and(|mark| mark == "B"));
    assert_eq!(directory_reports(&shell.transcript), [] as [&str; 0]);
    assert!(markers(&shell.transcript).iter().any(|mark| mark == "D;1"));
    // A PWD change completes at the next prompt, with reporting still absent.
    shell.cycle("cd /");
    assert_eq!(directory_reports(&shell.transcript), [] as [&str; 0]);
}

#[test]
fn environment_diagnostics_allow_only_named_fixture_values() {
    assert_eq!(
        environment_diagnostics(
            "SECRET=redacted\nHUTERM_TEST=visible\nPATH=/bin\nZDOTDIR=/tmp\nENV=\nHISTFILE=\nXDG_DATA_DIRS=\nHOME=/tmp\nXDG_CONFIG_HOME=redacted\ncontinued secret\n"
        ),
        "HUTERM_TEST=visible\nPATH=/bin\nZDOTDIR=/tmp\nENV=\nHISTFILE=\nXDG_DATA_DIRS=\nHOME=/tmp"
    );
}
#[test]
fn native_elvish() {
    run_shell("elvish");
}

fn assert_stream(shell: &mut Shell, name: &str, input: &str, status: &str) {
    shell.cycle(input);
    if name != "elvish" {
        shell.until(|bytes| {
            let marks = markers(bytes);
            marks
                .iter()
                .position(|mark| mark == "C")
                .is_some_and(|index| {
                    marks[index..].iter().any(|value| value == "B")
                })
        });
    }
    let expected = if name == "elvish" {
        vec!["C".into(), format!("D;{status}"), "A".into()]
    } else {
        vec!["C".into(), format!("D;{status}"), "A".into(), "B".into()]
    };
    let marks = markers(&shell.transcript);
    let marks = &marks[marks.iter().position(|mark| mark == "C").unwrap()..];
    assert_eq!(
        marks,
        expected,
        "{name}: {:?}",
        String::from_utf8_lossy(&shell.transcript)
    );
    if name == "nu" {
        let reports = directory_reports(&shell.transcript);
        assert_eq!(reports.len(), 1, "one Nushell OSC 7 stream per prompt");
        assert!(reports[0].starts_with("kitty-shell-cwd://"));
    }
}

fn assert_environment(name: &str, env: &str, path_enabled: bool) {
    let env: std::collections::BTreeMap<_, _> = env
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    for key in OWNED {
        if !matches!(
            *key,
            "HUTERM_BIN_DIR" | "HUTERM_RESOURCES_DIR" | "HUTERM_SHELL_FEATURES"
        ) {
            assert!(!env.contains_key(key), "{name}: leaked {key}");
        }
    }
    assert_eq!(
        env.get("XDG_DATA_DIRS"),
        Some(&""),
        "{name}: XDG_DATA_DIRS restoration"
    );
    if matches!(name, "bash" | "/bin/bash") {
        assert_eq!(env.get("ENV"), Some(&""), "{name}: ENV restoration");
        assert_eq!(
            env.get("HISTFILE"),
            Some(&""),
            "{name}: HISTFILE restoration"
        );
    }
    let bin = env["HUTERM_BIN_DIR"];
    assert_eq!(
        env["PATH"]
            .split(':')
            .filter(|component| *component == bin)
            .count(),
        usize::from(path_enabled),
        "{name}: PATH component count"
    );
}

fn run_path_and_nested(name: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    for already_present in [false, true] {
        let (home, mut command) = fixture(name, false);
        command.program.clone_from(&binary);
        if already_present {
            command.environment.push((
                "PATH".into(),
                format!(
                    "{}:{}",
                    std::env::var("PATH").unwrap(),
                    std::env::current_exe()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .display()
                ),
            ));
        } else {
            command.shell_integration.features =
                huterm_protocol::ShellFeatures::default();
        }
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        let env = shell.capture_env(name, &home);
        assert_environment(name, &env, already_present);
    }
    let (home, mut command) = fixture(name, false);
    command.program.clone_from(&binary);
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    let rc = match name {
        "bash" | "/bin/bash" => ".bashrc",
        "zsh" => ".zshrc",
        "fish" => ".config/fish/config.fish",
        "nu" => ".config/nushell/config.nu",
        "elvish" => ".config/elvish/rc.elv",
        _ => unreachable!(),
    };
    let mut startup = fs::read_to_string(home.0.join(rc)).unwrap();
    if name == "elvish" {
        startup =
            "set edit:prompt = { put 'READY> ' }\nset edit:rprompt = {}\n"
                .into();
    }
    home.write(
        rc,
        &format!(
            "{} NESTED_START\n{startup}",
            if name == "nu" { "print" } else { "echo" }
        ),
    );
    shell.transcript.clear();
    shell.send(&format!(
        "{}'{}'\n",
        if name == "nu" { "^" } else { "" },
        binary.display()
    ));
    shell.until(|bytes| {
        String::from_utf8_lossy(bytes).contains("NESTED_START\r\n")
    });
    shell.until(|bytes| {
        String::from_utf8_lossy(bytes)
            .rsplit("NESTED_START\r\n")
            .next()
            .unwrap()
            .contains("READY>")
    });
    shell.plain(&format!(
        "/usr/bin/env {} '{}'",
        if name == "nu" { "out>" } else { ">" },
        home.0.join("nested-env").display()
    ));
    let env = fs::read_to_string(home.0.join("nested-env")).unwrap();
    assert!(
        !env.lines()
            .any(|line| line.starts_with("HUTERM_SHELL_INTEGRATION="))
    );
    if matches!(name, "bash" | "/bin/bash" | "zsh" | "elvish") {
        assert!(
            markers(&shell.transcript).is_empty(),
            "{name}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
    }
    eprintln!(
        "native {name}: no-feature, existing PATH, nested plain cases ran"
    );
}

#[test]
fn native_zsh_path_and_nested() {
    run_path_and_nested("zsh");
}
#[test]
fn native_bash_path_and_nested() {
    run_path_and_nested("bash");
}
#[cfg(target_os = "macos")]
#[test]
fn native_apple_bash_path_and_nested() {
    run_path_and_nested("/bin/bash");
}
#[test]
fn native_fish_path_and_nested() {
    run_path_and_nested("fish");
}
#[test]
fn native_nushell_path_and_nested() {
    run_path_and_nested("nu");
}
#[test]
fn native_elvish_path_and_nested() {
    run_path_and_nested("elvish");
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run_failures(name: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    for damage in ["body syntax", "body runtime", "bootstrap"] {
        let assets = Directory::new();
        let original = resources(
            &std::env::current_exe().unwrap(),
            cfg!(target_os = "macos"),
        )
        .unwrap();
        copy_tree(
            &original.join("shell-integration"),
            &assets.0.join("shell-integration"),
        );
        let (home, mut command) = fixture(name, false);
        command.program.clone_from(&binary);
        let (bootstrap, body) = match name {
            "bash" | "/bin/bash" => ("bash/bootstrap.bash", "bash/huterm.bash"),
            "zsh" => ("zsh/.zshenv", "zsh/huterm.zsh"),
            "fish" => (
                "fish/xdg/fish/vendor_conf.d/huterm.fish",
                "fish/huterm.fish",
            ),
            "nu" => (
                "nushell/xdg/nushell/vendor/autoload/huterm.nu",
                "nushell/huterm.nu",
            ),
            "elvish" => (
                "elvish/xdg/elvish/lib/huterm-integration.elv",
                "elvish/huterm.elv",
            ),
            _ => unreachable!(),
        };
        let target =
            assets
                .0
                .join("shell-integration")
                .join(if damage == "bootstrap" {
                    bootstrap
                } else {
                    body
                });
        let invalid = if damage == "body runtime" {
            match name {
                "elvish" => "fail broken\n",
                "nu" => "error make {msg: broken}\n",
                _ => "return 1\n",
            }
        } else {
            "if\n"
        };
        if damage == "bootstrap" {
            let mut text = fs::read_to_string(&target).unwrap();
            text.push_str(invalid);
            fs::write(target, text).unwrap();
        } else {
            fs::write(target, invalid).unwrap();
        }
        let planned = launch_plan(&command, Some(assets.0.clone()));
        let process =
            crate::pty::spawn_with_plan_isolated(&command, planned).unwrap();
        let mut shell = Shell::from_process(&command, process);
        shell.prompt();
        let first = String::from_utf8_lossy(&shell.transcript).into_owned();
        assert!(home.0.join("order").is_file(), "{name} {damage}: {first}");
        if damage != "bootstrap" {
            assert_eq!(
                first
                    .matches("huterm: shell integration failed to load:")
                    .count(),
                1,
                "{name} {damage}: {first}"
            );
        }
        // Prove startup remains usable and no message repeats at another prompt.
        shell.transcript.clear();
        shell.send("/usr/bin/printf 'ALIVE\\n'\n");
        shell.until(|bytes| {
            String::from_utf8_lossy(bytes).contains("ALIVE\r\n")
        });
        shell.until(|bytes| {
            String::from_utf8_lossy(bytes)
                .rsplit("ALIVE\r\n")
                .next()
                .unwrap()
                .contains("READY>")
        });
        assert!(
            !String::from_utf8_lossy(&shell.transcript)
                .contains("failed to load")
        );
        eprintln!(
            "native {name}: {damage} preserves startup, reports once, remains usable"
        );
    }
}

#[test]
fn native_zsh_failures() {
    run_failures("zsh");
}
#[test]
fn native_bash_failures() {
    run_failures("bash");
}
#[cfg(target_os = "macos")]
#[test]
fn native_apple_bash_failures() {
    run_failures("/bin/bash");
}
#[test]
fn native_fish_failures() {
    run_failures("fish");
}
#[test]
fn native_nushell_failures() {
    run_failures("nu");
}
#[test]
fn native_elvish_failures() {
    run_failures("elvish");
}

fn run_off(name: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    let (home, mut command) = fixture(name, false);
    command.program = binary;
    command.shell_integration = huterm_protocol::ShellIntegration::OFF;
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    let env = shell.capture_env(name, &home);
    assert_environment(name, &env, false);
    assert!(
        !env.lines()
            .any(|line| line.starts_with("HUTERM_SHELL_FEATURES="))
    );
    if matches!(name, "bash" | "/bin/bash" | "zsh" | "elvish") {
        assert_eq!(markers(&shell.transcript), [] as [String; 0]);
    }
    eprintln!(
        "native {name}: off preserves startup and the original environment"
    );
}

#[test]
fn native_zsh_off() {
    run_off("zsh");
}
#[test]
fn native_bash_off() {
    run_off("bash");
}
#[cfg(target_os = "macos")]
#[test]
fn native_apple_bash_off() {
    run_off("/bin/bash");
}
#[test]
fn native_fish_off() {
    run_off("fish");
}
#[test]
fn native_nushell_off() {
    run_off("nu");
}
#[test]
fn native_elvish_off() {
    run_off("elvish");
}

#[test]
fn native_bash_and_zsh_avoid_framework_markers() {
    for name in ["bash", "/bin/bash", "zsh"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for marker in [
            "\x1b]133;A\x07",
            "\\e]133;A\\a",
            "\\033]133;A\\a",
            "\\x1b]133;A\\a",
        ] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            home.write(
                if name == "zsh" { ".zshrc" } else { ".bashrc" },
                &format!(
                    "{}='{marker}READY> '\n",
                    if name == "zsh" { "PROMPT" } else { "PS1" }
                ),
            );
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            shell.plain("true");
            let marks = markers(&shell.transcript);
            assert!(
                !marks
                    .iter()
                    .any(|mark| mark == "C" || mark.starts_with("D;")),
                "{name} {marker:?}: {marks:?}"
            );
            assert!(
                marks.iter().filter(|mark| *mark == "A").count() <= 1,
                "{name} {marker:?}: {marks:?}"
            );
            assert!(
                String::from_utf8_lossy(&shell.transcript)
                    .contains("]7;kitty-shell-cwd://")
            );
        }
    }
}

#[test]
fn native_bash_startup_options_and_zdotdir() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for option in ["--norc", "--noprofile", "--rcfile", "--init-file"] {
            let (home, mut command) = fixture(name, option == "--noprofile");
            command.program.clone_from(&binary);
            command.arguments.insert(0, option.into());
            if matches!(option, "--rcfile" | "--init-file") {
                home.write(
                    "custom-rc",
                    "echo custom > \"$HOME/order\"\nPS1='READY> '\n",
                );
                command.arguments.insert(
                    1,
                    home.0.join("custom-rc").to_string_lossy().into_owned(),
                );
            }
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            if matches!(option, "--rcfile" | "--init-file") {
                assert_eq!(
                    fs::read_to_string(home.0.join("order")).unwrap(),
                    "custom\n"
                );
            } else {
                assert!(!home.0.join("order").exists(), "{name} {option}");
            }
        }
    }
    if let Some(binary) = executable("zsh") {
        let (home, mut command) = fixture("zsh", true);
        fs::create_dir(home.0.join("zdot")).unwrap();
        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            fs::rename(home.0.join(name), home.0.join("zdot").join(name))
                .unwrap();
        }
        command.program = binary;
        command.environment.push((
            "ZDOTDIR".into(),
            home.0.join("zdot").to_string_lossy().into_owned(),
        ));
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert_eq!(
            fs::read_to_string(home.0.join("order")).unwrap(),
            ".zshenv\n.zprofile\n.zshrc\n.zlogin\n"
        );
    }
}

#[test]
fn native_directory_reports_preserve_literal_paths_and_skip_controls() {
    for name in ["bash", "/bin/bash", "zsh", "fish", "nu", "elvish"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        let target = home.0.join("literal path #?%20");
        fs::create_dir(&target).unwrap();
        let target = target.canonicalize().unwrap();
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        shell.cycle(&format!("cd '{}'", target.display()));
        assert_eq!(
            shell.directory.as_ref().unwrap().path(),
            target.to_string_lossy(),
            "{name}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        assert!(shell.directory.as_ref().unwrap().is_local());
        if matches!(name, "bash" | "/bin/bash" | "zsh" | "nu" | "elvish") {
            for control in ['\u{1}', '\u{7f}', '\u{80}', '\u{9f}'] {
                let target = home.0.join(format!("control{control}"));
                fs::create_dir(&target).unwrap();
                if name == "nu" {
                    // Nushell's cd strips controls. Starting in the directory
                    // bypasses that resolver and exercises the real PWD hook.
                    command.working_directory = target.canonicalize().unwrap();
                    let mut shell = Shell::spawn(&command);
                    shell.prompt();
                    assert!(
                        directory_reports(&shell.transcript).is_empty(),
                        "Nushell reported a control path at startup: {:?}",
                        directory_reports(&shell.transcript)
                    );
                    shell.cycle("^/bin/pwd");
                    assert!(
                        String::from_utf8_lossy(&shell.transcript).contains(
                            &format!(
                                "{}\r\n",
                                command.working_directory.display()
                            )
                        ),
                        "Nushell did not start in {target:?}: {:?}",
                        String::from_utf8_lossy(&shell.transcript)
                    );
                    assert!(
                        directory_reports(&shell.transcript).is_empty(),
                        "Nushell reported a control path after a command: {:?}",
                        directory_reports(&shell.transcript)
                    );
                    continue;
                }
                // Expand a variable instead of typing editor control keys.
                let mut encoded = [0; 4];
                let mut escaped = String::new();
                for byte in control.encode_utf8(&mut encoded).bytes() {
                    use std::fmt::Write as _;
                    write!(escaped, "\\{byte:03o}").unwrap();
                }
                let expression = if name == "elvish" {
                    format!(
                        "cd '{}'\"\\u{:04x}\"",
                        home.0.join("control").display(),
                        u32::from(control)
                    )
                } else {
                    format!(
                        "cd \"{}$(/usr/bin/printf '{escaped}')\"",
                        home.0.join("control").display()
                    )
                };
                shell.cycle(&expression);
                assert!(
                    !String::from_utf8_lossy(&shell.transcript)
                        .contains("]7;kitty-shell-cwd://"),
                    "{name} {control:?}: {:?}",
                    directory_reports(&shell.transcript)
                );
            }
        }
    }
}
