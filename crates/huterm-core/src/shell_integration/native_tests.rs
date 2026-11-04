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
            std::env::var_os("HUTERM_REQUIRE_SHELLS").is_none()
                // macOS CI requires Apple Bash; Linux requires Bash 5.
                || (name == "bash" && cfg!(target_os = "macos")),
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
        ("XDG_DATA_DIRS".into(), "/fixture/data".into()),
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
                directory.write(name, &format!("skip_global_compinit=1\nprint -r -- {name} >> \"$HOME/order\"\nPROMPT='READY> '\n"));
            }
        }
        "fish" => {
            // Ubuntu snapd mutates XDG paths in a system vendor snippet. Shadow
            // it so unset/empty fixtures test restoration without distro policy.
            directory.write(".config/fish/conf.d/snapd.fish", "# No system snapd setup in this fixture.\n");
            directory.write(".config/fish/config.fish", "echo fish >> $HOME/order\nfunction fish_prompt; printf 'READY> '; end\n");
        },
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
        assert_restored(&command, &home, &env);
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
        assert_restored(&command, &home, &env);
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
    assert_restored(&command, &home, &env);
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

// Compare with this host's normal startup, including system vendor snippets.
fn assert_restored(
    command: &TerminalCommand,
    home: &Directory,
    integrated: &str,
) {
    let mut plain = command.clone();
    plain.shell_integration = huterm_protocol::ShellIntegration::OFF;
    let mut shell = Shell::spawn(&plain);
    shell.prompt();
    let baseline = shell.capture_env(
        command.program.file_name().unwrap().to_str().unwrap(),
        home,
    );
    let values = |text: &str| -> std::collections::BTreeMap<String, String> {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .filter(|(name, _)| {
                matches!(
                    *name,
                    "ZDOTDIR" | "ENV" | "HISTFILE" | "XDG_DATA_DIRS"
                )
            })
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    };
    assert_eq!(
        values(integrated),
        values(&baseline),
        "{}: startup environment restoration",
        command.program.display()
    );
}

fn finish(shell: &mut Shell) {
    shell.send("exit\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0; 4096];
    loop {
        assert!(Instant::now() < deadline, "shell did not exit");
        match shell.reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                shell
                    .waiter
                    .wait(Some(
                        deadline.saturating_duration_since(Instant::now()),
                    ))
                    .unwrap();
            }
            Err(error) => panic!("exit read: {error}"),
        }
    }
}

#[test]
fn native_bash_history_persists_and_preserves_ignorespace() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for login in [false, true] {
            for integrated in [false, true] {
                let (home, mut command) = fixture(name, login);
                command.program.clone_from(&binary);
                command.environment.retain(|(key, _)| {
                    !matches!(key.as_str(), "HISTFILE" | "ENV")
                });
                command
                    .environment
                    .push(("HISTCONTROL".into(), "ignorespace".into()));
                if !integrated {
                    command.shell_integration =
                        huterm_protocol::ShellIntegration::OFF;
                }
                home.write(".bash_history", "echo HUTERM_HISTORY_SEED\n");
                let mut shell = Shell::spawn(&command);
                shell.prompt();
                shell.plain("echo HUTERM_HISTORY_SAVED");
                shell.plain(" echo HUTERM_HISTORY_PRIVATE");
                finish(&mut shell);
                let history =
                    fs::read_to_string(home.0.join(".bash_history")).unwrap();
                assert!(
                    history.contains("echo HUTERM_HISTORY_SAVED"),
                    "{name} login={login} integrated={integrated}: history was not saved"
                );
                assert!(
                    !history.contains("HUTERM_HISTORY_PRIVATE"),
                    "{name}: ignorespace was weakened"
                );
                let mut shell = Shell::spawn(&command);
                shell.prompt();
                shell.plain("history > \"$HOME/seen-history\"");
                let seen =
                    fs::read_to_string(home.0.join("seen-history")).unwrap();
                assert!(
                    seen.contains("HUTERM_HISTORY_SEED")
                        && seen.contains("HUTERM_HISTORY_SAVED"),
                    "{name}: history was not read"
                );
                finish(&mut shell);
            }
        }
    }
}

#[test]
fn native_prompt_rebuilding_hooks_run_before_markers() {
    for name in ["bash", "/bin/bash", "zsh"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for framework in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            let prompt = if framework {
                "$'\\e]133;A\\aREADY> \\e]133;B\\a'"
            } else {
                "'READY> '"
            };
            let startup = if framework {
                let theme = format!(
                    "theme() {{ local code=$?; if [[ ${{_fixture_framework_running-0}} == 1 ]]; then printf '\\e]133;D;%s\\a' \"$code\"; fi; _fixture_framework_running=0; {}={prompt}; }}\n",
                    if name == "zsh" { "PROMPT" } else { "PS1" }
                );
                if name == "zsh" {
                    format!(
                        "{theme}framework_preexec() {{ printf '\\e]133;C\\a'; _fixture_framework_running=1; }}\npreexec_functions+=(framework_preexec)\nprecmd_functions+=(theme)\n"
                    )
                } else {
                    format!(
                        "{theme}PROMPT_COMMAND=theme\ntrap \"printf '\\e]133;C\\a'; _fixture_framework_running=1\" DEBUG\n"
                    )
                }
            } else if name == "zsh" {
                format!(
                    "theme() {{ PROMPT={prompt}; return 42; }}\nprecmd_functions+=(theme)\n"
                )
            } else {
                format!(
                    "theme() {{ PS1={prompt}; return 42; }}\nPROMPT_COMMAND=theme\n"
                )
            };
            home.write(
                if name == "zsh" { ".zshrc" } else { ".bashrc" },
                &startup,
            );
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            for _ in 0..2 {
                assert_stream(&mut shell, name, "false", "1");
            }
        }
    }
}

#[test]
fn native_absolute_shells_with_restricted_path_and_unset_environment() {
    for name in ["zsh", "fish", "nu", "elvish"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        command.environment.retain(|(key, _)| {
            !matches!(
                key.as_str(),
                "ZDOTDIR" | "ENV" | "HISTFILE" | "XDG_DATA_DIRS" | "PATH"
            )
        });
        let restricted = home.0.join("restricted-bin");
        fs::create_dir(&restricted).unwrap();
        for tool in [
            "locale", "hostname", "uname", "getconf", "tty", "false", "true",
        ] {
            if let Some(source) = resolve(
                Path::new(tool),
                &home.0,
                std::env::var_os("PATH").as_deref(),
            ) {
                std::os::unix::fs::symlink(source, restricted.join(tool))
                    .unwrap();
            }
        }
        let search = restricted.to_string_lossy().into_owned();
        assert!(
            resolve(Path::new(name), &home.0, Some(OsStr::new(&search)))
                .is_none()
        );
        command.environment.push(("PATH".into(), search));
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert!(
            !String::from_utf8_lossy(&shell.transcript)
                .contains("failed to load"),
            "{name}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        let env = shell.capture_env(name, &home);
        assert_environment(name, &env, true);
        assert_restored(&command, &home, &env);
        assert_stream(
            &mut shell,
            name,
            if name == "nu" { "^false" } else { "false" },
            "1",
        );
    }
}

#[test]
fn native_xdg_startup_changes_survive_restoration() {
    for name in ["fish", "nu", "elvish"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for original in [None, Some(""), Some("/fixture/original")] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            command
                .environment
                .retain(|(key, _)| key != "XDG_DATA_DIRS");
            if let Some(value) = original {
                command
                    .environment
                    .push(("XDG_DATA_DIRS".into(), value.into()));
            }
            match name {
                "fish" => home.write(".config/fish/conf.d/user.fish", "set -gx XDG_DATA_DIRS /user/flatpak:$XDG_DATA_DIRS\n"),
                "nu" => home.write(".config/nushell/env.nu", "$env.XDG_DATA_DIRS = '/user/flatpak:' + ($env.XDG_DATA_DIRS? | default '')\n"),
                "elvish" => {
                    let rc = fs::read_to_string(home.0.join(".config/elvish/rc.elv")).unwrap();
                    home.write(".config/elvish/rc.elv", &format!("set-env XDG_DATA_DIRS /user/flatpak:$E:XDG_DATA_DIRS\n{rc}"));
                },
                _ => unreachable!(),
            }
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            let env = shell.capture_env(name, &home);
            let xdg = env
                .lines()
                .find_map(|line| line.strip_prefix("XDG_DATA_DIRS="))
                .expect("startup XDG_DATA_DIRS was discarded");
            assert!(
                xdg.starts_with("/user/flatpak:"),
                "{name}: lost startup XDG change"
            );
            assert!(
                !xdg.contains("shell-integration"),
                "{name}: injection entry leaked"
            );
            if original.is_none_or(str::is_empty) {
                assert!(
                    xdg.contains("/usr/local/share:/usr/share"),
                    "{name}: missing XDG defaults"
                );
            }
        }
    }
}

#[test]
fn native_nushell_partial_install_is_rolled_back() {
    let Some(binary) = executable("nu") else {
        return;
    };
    let assets = Directory::new();
    let original =
        resources(&std::env::current_exe().unwrap(), cfg!(target_os = "macos"))
            .unwrap();
    copy_tree(
        &original.join("shell-integration"),
        &assets.0.join("shell-integration"),
    );
    let body = assets.0.join("shell-integration/nushell/huterm.nu");
    let mut text = fs::read_to_string(&body).unwrap();
    text.push_str("\nerror make {msg: 'partial install'}\n");
    fs::write(body, text).unwrap();
    let (home, mut command) = fixture("nu", false);
    command.program = binary;
    let config = home.0.join(".config/nushell/config.nu");
    let mut startup = fs::read_to_string(&config).unwrap();
    startup.push_str("$env.config.hooks.pre_prompt = []\n$env.config.hooks.env_change.PWD = []\n");
    fs::write(config, startup).unwrap();
    let process = crate::pty::spawn_with_plan_isolated(
        &command,
        launch_plan(&command, Some(assets.0.clone())),
    )
    .unwrap();
    let mut shell = Shell::from_process(&command, process);
    shell.prompt();
    assert_eq!(
        String::from_utf8_lossy(&shell.transcript)
            .matches("huterm: shell integration failed to load:")
            .count(),
        1
    );
    shell.plain("[$env.config.shell_integration.osc7 ($env.config.hooks.pre_prompt | length) ($env.config.hooks.env_change.PWD | length) ($env.HUTERM_BIN_DIR in $env.PATH)] | to nuon | save state");
    assert_eq!(
        fs::read_to_string(home.0.join("state")).unwrap().trim(),
        "[true, 0, 0, false]"
    );
    assert!(
        !String::from_utf8_lossy(&shell.transcript).contains("failed to load")
    );
}

#[test]
fn native_bash_explicit_empty_rcfile_reads_no_startup_file() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        command.arguments = vec!["--rcfile".into(), String::new()];
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert!(
            !home.0.join("order").exists(),
            "{name}: empty rcfile fell back to .bashrc"
        );
        assert_stream(&mut shell, name, "true", "0");
    }
}

#[test]
fn native_xdg_restores_unset_and_empty_values() {
    for name in ["fish", "nu", "elvish"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for original in [None, Some("")] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            command
                .environment
                .retain(|(key, _)| key != "XDG_DATA_DIRS");
            if let Some(value) = original {
                command
                    .environment
                    .push(("XDG_DATA_DIRS".into(), value.into()));
            }
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            let env = shell.capture_env(name, &home);
            assert_environment(name, &env, true);
            assert_restored(&command, &home, &env);
        }
    }
}

#[test]
fn native_bash_prompt_command_arrays_preserve_shell_version_behavior() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        home.write(
            ".bashrc",
            "theme() { PS1='READY> '; }\nPROMPT_COMMAND=(theme false)\n",
        );
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert_stream(&mut shell, name, "true", "0");
        assert_stream(&mut shell, name, "false", "1");
    }
}
