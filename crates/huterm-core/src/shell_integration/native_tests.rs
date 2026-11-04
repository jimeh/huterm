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
    /// Waits for Huterm's input mark, for prompts a system startup file
    /// replaced, such as Debian's `/etc/bash.bashrc` setting `PS1`.
    fn marked_prompt(&mut self) {
        self.until(|bytes| markers(bytes).iter().any(|mark| mark == "B"));
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
    /// Runs one input line exactly as typed and waits for the next prompt.
    fn line(&mut self, input: &str) {
        self.transcript.clear();
        self.send(&format!("{input}\n"));
        self.until(|bytes| {
            String::from_utf8_lossy(bytes)
                .split_once('\n')
                .is_some_and(|(_, rest)| rest.contains("READY>"))
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
            environment: &|key| {
                command
                    .environment
                    .iter()
                    .rev()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| OsString::from(value))
            },
            is_file: &Path::is_file,
        },
    )
}

/// The executable a shell finds for `program` through `search`, like portable-pty.
fn resolve(
    program: &Path,
    cwd: &Path,
    search: Option<&OsStr>,
) -> Option<PathBuf> {
    let executable = |file: &Path| {
        nix::unistd::access(file, nix::unistd::AccessFlags::X_OK).is_ok()
            && file.is_file()
    };
    if program.is_absolute() {
        return executable(program).then(|| program.to_owned());
    }
    std::env::split_paths(search?)
        .map(|part| cwd.join(part).join(program))
        .find(|file| executable(file))
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
        if bash_version(&version.stdout) < (4, 4) {
            result = None;
            eprintln!("shell integration: Bash 4.4 or later is unavailable");
        }
    }

    if result.is_none() {
        assert!(
            std::env::var_os("HUTERM_REQUIRE_SHELLS").is_none()
                // macOS CI requires Apple Bash; Linux requires Bash 4.4 or later.
                || (name == "bash" && cfg!(target_os = "macos")),
            "required shell missing: {name}"
        );
        eprintln!("skipping shell integration: {name} missing");
    }
    result
}

/// The major and minor version from `bash --version` output.
fn bash_version(output: &[u8]) -> (u32, u32) {
    let text = String::from_utf8_lossy(output);
    let version = text
        .split_once("version ")
        .map_or("", |(_, rest)| rest.split(['(', ' ']).next().unwrap_or(""));
    let mut parts = version.split('.').map(|part| part.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Bash 5.1 added the array form of `PROMPT_COMMAND`.
fn prompt_command_arrays(binary: &Path) -> bool {
    let version = std::process::Command::new(binary)
        .env_clear()
        .arg("--version")
        .output()
        .unwrap();
    bash_version(&version.stdout) >= (5, 1)
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
    // Prompt decorations must reach the terminal as escapes, not as text.
    assert!(
        !String::from_utf8_lossy(&shell.transcript).contains("_huterm_"),
        "{name}: {:?}",
        String::from_utf8_lossy(&shell.transcript)
    );
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
            if option == "--norc" {
                shell.prompt();
                // Injection needs an rcfile, so --norc starts bash plain.
                assert!(
                    !String::from_utf8_lossy(&shell.transcript)
                        .contains("]133;"),
                    "{name} --norc was injected"
                );
            } else {
                // Debian-family bash reads its system bashrc, which may set
                // PS1, before the bootstrap emulates a login start.
                shell.marked_prompt();
            }
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
                let theme = if name == "zsh" {
                    theme
                } else {
                    theme.replace("; }", "; _fixture_framework_armed=1; }")
                };
                if name == "zsh" {
                    format!(
                        "{theme}framework_preexec() {{ printf '\\e]133;C\\a'; _fixture_framework_running=1; }}\npreexec_functions+=(framework_preexec)\nprecmd_functions+=(theme)\n"
                    )
                } else {
                    format!(
                        "{theme}{}",
                        r#"PROMPT_COMMAND=theme
_fixture_framework_armed=1
fixture_framework_debug() {
    local entry
    for entry in "${PROMPT_COMMAND[@]}"; do
        case $'\n'"$entry"$'\n' in
            *$'\n'"$BASH_COMMAND"$'\n'*) return 0 ;;
        esac
    done
    if [[ $_fixture_framework_armed == 1 ]]; then
        _fixture_framework_armed=0
        printf '\e]133;C\a'
        _fixture_framework_running=1
    fi
}
trap 'fixture_framework_debug "$_"' DEBUG
"#
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
        // Debian-family bash still reads its system bashrc, which replaces the
        // fixture's PS1 even in a plain start.
        shell.marked_prompt();
        shell.line("PS1='READY> '");
        assert!(
            !home.0.join("order").exists(),
            "{name}: empty rcfile fell back to .bashrc"
        );
        assert_stream(&mut shell, name, "true", "0");
    }
}

#[test]
fn native_bash_err_traps_ignore_huterm_status_restores() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        // The second case adds a DEBUG trap, which Huterm's own trap wraps.
        for debug in ["", "trap ':' DEBUG\n"] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            // A startup file that ends with a failed && list leaves a nonzero
            // status without firing ERR in a plain start.
            home.write(
                ".bashrc",
                &format!(
                    "PS1='READY> '\ntrap 'echo ERR_FIRED' ERR\n{debug}[ -f \"$HOME/missing\" ] && . \"$HOME/missing\"\n"
                ),
            );
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            let count = |bytes: &[u8]| {
                String::from_utf8_lossy(bytes).matches("ERR_FIRED").count()
            };
            assert_eq!(
                count(&shell.transcript),
                0,
                "{name} {debug:?}: startup {:?}",
                String::from_utf8_lossy(&shell.transcript)
            );
            let failed = shell.cycle("false");
            assert_eq!(count(&failed), 1, "{name} {debug:?}: false");
            let after = shell.cycle("echo after");
            assert_eq!(count(&after), 0, "{name} {debug:?}: after");
        }
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

#[test]
fn native_zsh_prompt_survives_replaced_and_later_line_init_widgets() {
    let Some(binary) = executable("zsh") else {
        return;
    };
    for later_hook in [false, true] {
        let (home, mut command) = fixture("zsh", false);
        command.program.clone_from(&binary);
        home.write(".zshrc", &format!(
            "PROMPT='READY> '\nfixture_line_init() {{ print -rn -- WIDGET; }}\nzle -N zle-line-init fixture_line_init\n{}",
            if later_hook { "autoload -Uz add-zle-hook-widget\nfixture_later() { print -rn -- LATER; }\nadd-zle-hook-widget line-init fixture_later\n" } else { "" }
        ));
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        shell.until(|bytes| {
            String::from_utf8_lossy(bytes).contains(if later_hook {
                "LATER"
            } else {
                "WIDGET"
            })
        });
        assert_eq!(
            markers(&shell.transcript),
            ["A", "B"],
            "later_hook={later_hook}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        assert_stream(&mut shell, "zsh", "false", "1");
    }
}

#[test]
fn native_zsh_prompt_substitution_expands_once_like_plain_start() {
    let Some(binary) = executable("zsh") else {
        return;
    };
    let mut counts = Vec::new();
    for integrated in [false, true] {
        let (home, mut command) = fixture("zsh", false);
        command.program.clone_from(&binary);
        if !integrated {
            command.shell_integration = huterm_protocol::ShellIntegration::OFF;
        }
        home.write(".zshrc", "setopt prompt_subst\nfixture_prompt() { print tick >> \"$HOME/expansions\"; print -rn -- 'READY> '; }\nPROMPT='$(fixture_prompt)'\nfixture_expanded() { print -rn -- FIXTURE_EXPANDED; }\nautoload -Uz add-zle-hook-widget\nadd-zle-hook-widget line-init fixture_expanded\n");
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        shell.until(|bytes| {
            String::from_utf8_lossy(bytes).contains("FIXTURE_EXPANDED")
        });
        for _ in 0..2 {
            shell.plain("true");
            shell.until(|bytes| {
                String::from_utf8_lossy(bytes).contains("FIXTURE_EXPANDED")
            });
        }
        counts.push(
            fs::read_to_string(home.0.join("expansions"))
                .unwrap()
                .lines()
                .count(),
        );
    }
    assert_eq!(counts, [3, 3]);
}

#[test]
fn native_zsh_missing_body_reports_exactly_one_line() {
    let Some(binary) = executable("zsh") else {
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
    let body = assets.0.join("shell-integration/zsh/huterm.zsh");
    fs::remove_file(&body).unwrap();
    let (_home, mut command) = fixture("zsh", false);
    command.program = binary;
    let process = crate::pty::spawn_with_plan_isolated(
        &command,
        launch_plan(&command, Some(assets.0.clone())),
    )
    .unwrap();
    let mut shell = Shell::from_process(&command, process);
    shell.prompt();
    let text = String::from_utf8_lossy(&shell.transcript);
    let errors: Vec<_> = text
        .lines()
        .filter(|line| line.contains("huterm.zsh"))
        .collect();
    assert_eq!(
        errors,
        [format!(
            "huterm: shell integration failed to load: {}",
            body.display()
        )]
    );
    shell.plain("true");
    assert!(
        !String::from_utf8_lossy(&shell.transcript).contains("failed to load")
    );
}

#[test]
fn native_bash_preserves_last_argument_status_and_pipeline() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let mut states = Vec::new();
        for integrated in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            if !integrated {
                command.shell_integration =
                    huterm_protocol::ShellIntegration::OFF;
            }
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            shell.plain("mkdir -p d && cd $_; printf '%s\\n' \"${PWD##*/}\" > \"$HOME/directory\"");
            shell.plain(
                "echo foo bar; printf '%s\\n' \"$_\" > \"$HOME/argument\"",
            );
            // Separate input cycles also exercise Bash's saved prompt state.
            shell.transcript.clear();
            shell.send("false | true\n");
            shell.prompt();
            shell.plain("printf '%s:%s\\n' \"$?\" \"${PIPESTATUS[*]}\" > \"$HOME/pipeline\"");
            states
                .push(["directory", "argument", "pipeline"].map(|file| {
                    fs::read_to_string(home.0.join(file)).unwrap()
                }));
        }
        assert_eq!(
            states[0],
            ["d\n", "bar\n", "0:1 0\n"],
            "{name}: plain baseline"
        );
        // Jim accepted Apple Bash 3.2's DEBUG-trap PIPESTATUS limitation on
        // 2026-11-04. Keep the exception explicit while checking the other state.
        let expected = if name == "/bin/bash" && cfg!(target_os = "macos") {
            ["d\n", "bar\n", "0:0\n"]
        } else {
            ["d\n", "bar\n", "0:1 0\n"]
        };
        assert_eq!(states[1], expected, "{name}: integrated shell state");
    }
}

#[test]
fn native_bash_user_debug_trap_keeps_status_and_simple_command_firings() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let mut logs = Vec::new();
        for integrated in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            if !integrated {
                command.shell_integration =
                    huterm_protocol::ShellIntegration::OFF;
            }
            home.write(".bashrc", "PS1='READY> '\ntrap 'printf \"%s:%s\\n\" \"$?\" \"$BASH_COMMAND\" >> \"$HOME/debug\"' DEBUG\n");
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            shell.plain("false; true; true");
            let log = fs::read_to_string(home.0.join("debug")).unwrap();
            logs.push(
                log.lines()
                    .filter(|line| {
                        line.ends_with(":false") || line.ends_with(":true")
                    })
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            logs[0],
            ["0:false", "1:true", "0:true"],
            "{name}: plain baseline"
        );
        assert_eq!(logs[1], logs[0], "{name}: DEBUG trap changed");
    }
}

#[test]
fn native_bash_ps0_works_without_promptvars() {
    let Some(binary) = executable("bash") else {
        return;
    };
    let (home, mut command) = fixture("bash", false);
    command.program = binary;
    home.write(".bashrc", "PS1='READY> '\nshopt -u promptvars\n");
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    assert_stream(&mut shell, "bash", "false", "1");
    assert!(!String::from_utf8_lossy(&shell.transcript).contains("${_huterm"));
}

#[test]
fn native_bash_lazy_preexec_installer_keeps_first_and_later_callbacks() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        // Huterm-authored emulation of bash-preexec's public callback arrays
        // and its lazy-install boundary: the first and last prompt-command
        // entry names, a traced installer that keeps a prior DEBUG trap as a
        // preexec callback, and prompt-command filtering. It reads no history.
        home.write(
            ".bashrc",
            r#"PS1='READY> '
bash_preexec_imported=fixture
preexec_functions=(fixture_user_preexec)
precmd_functions=(fixture_user_precmd)
fixture_user_preexec() { printf 'preexec\n' >> "$HOME/callbacks"; }
fixture_user_precmd() { local code=$?; printf 'precmd:%s\n' "$code" >> "$HOME/statuses"; }
fixture_status() { return "$1"; }
fixture_in_prompt_command() {
    local entry command=$1 IFS=$'\n;'
    command=${command#"${command%%[![:space:]]*}"}
    command=${command%"${command##*[![:space:]]}"}
    set -f
    for entry in $PROMPT_COMMAND; do
        entry=${entry#"${entry%%[![:space:]]*}"}
        entry=${entry%"${entry##*[![:space:]]}"}
        [[ $entry != "$command" ]] || { set +f; return 0; }
    done
    set +f
    return 1
}
fixture_debug() {
    if [[ ${fixture_armed-0} == 1 ]]; then
        fixture_armed=0
        fixture_in_prompt_command "$BASH_COMMAND" && return 0
        local callback
        for callback in "${preexec_functions[@]}"; do
            fixture_status "$fixture_last_status"
            "$callback" "$BASH_COMMAND"
        done
    fi
    return 0
}
__bp_precmd_invoke_cmd() {
    fixture_last_status=$?
    local callback
    for callback in "${precmd_functions[@]}"; do
        fixture_status "$fixture_last_status"
        "$callback"
    done
    return "$fixture_last_status"
}
__bp_interactive_mode() { fixture_armed=1; }
fixture_install() {
    local prior words=()
    prior=$(trap -p DEBUG)
    eval "words=($prior)"
    if [[ -n ${words[2]-} ]]; then
        eval "fixture_original_debug() { ${words[2]}
}"
        preexec_functions+=(fixture_original_debug)
    fi
    trap 'fixture_debug "$_"' DEBUG
    PROMPT_COMMAND=__bp_precmd_invoke_cmd$'\n'${PROMPT_COMMAND//fixture_install/}$'\n__bp_interactive_mode'
    __bp_precmd_invoke_cmd
    __bp_interactive_mode
}
declare -ft fixture_install
fixture_theme() { PS1='READY> '; }
PROMPT_COMMAND=$'fixture_theme\nfixture_install'
"#,
        );
        let mut shell = Shell::spawn(&command);
        shell.until(|bytes| markers(bytes).last().is_some_and(|m| m == "B"));
        shell.transcript.clear();
        shell.send("\n");
        shell.until(|bytes| markers(bytes).last().is_some_and(|m| m == "B"));
        assert_eq!(
            markers(&shell.transcript),
            ["A", "B"],
            "{name}: empty input after lazy installation"
        );
        assert!(
            !home.0.join("callbacks").exists(),
            "{name}: empty input ran preexec"
        );
        for (input, status) in [("true", "0"), ("false", "1"), ("true", "0")] {
            assert_stream(&mut shell, name, input, status);
        }
        assert_eq!(
            fs::read_to_string(home.0.join("callbacks")).unwrap(),
            "preexec\npreexec\npreexec\n",
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(home.0.join("statuses")).unwrap(),
            "precmd:0\nprecmd:0\nprecmd:0\nprecmd:1\nprecmd:0\n",
            "{name}"
        );
    }
}

#[test]
fn native_bash_empty_input_has_no_execution_or_completion_marks() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        home.write(".bashrc", "PS1='READY> '\nHISTCONTROL=ignorespace\n");
        let mut shell = Shell::spawn(&command);
        shell.until(|bytes| markers(bytes).last().is_some_and(|m| m == "B"));
        // Check empty input before the first command as well as between commands.
        for input in ["", "false", "", " true", ""] {
            if input.is_empty() {
                shell.transcript.clear();
                shell.send("\n");
                shell.until(|bytes| {
                    markers(bytes).last().is_some_and(|m| m == "B")
                });
                assert_eq!(markers(&shell.transcript), ["A", "B"], "{name}");
            } else {
                assert_stream(
                    &mut shell,
                    name,
                    input,
                    if input == "false" { "1" } else { "0" },
                );
                shell.until(|bytes| {
                    markers(bytes).last().is_some_and(|m| m == "B")
                });
            }
        }
    }
}

#[test]
fn native_bash_user_debug_trap_preserves_extdebug_skips() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for integrated in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            if !integrated {
                command.shell_integration =
                    huterm_protocol::ShellIntegration::OFF;
            }
            home.write(".bashrc", r#"PS1='READY> '
shopt -s extdebug
fixture_debug() {
    local code=$?
    case " ${FUNCNAME[*]} $BASH_COMMAND" in
        *' _huterm_bash_'*) printf '%s\n' "$BASH_COMMAND" >> "$HOME/internals" ;;
    esac
    case $BASH_COMMAND in
        'false'|'true') printf '%s:%s\n' "$code" "$BASH_COMMAND" >> "$HOME/commands" ;;
        'printf SKIP'*) return 1 ;;
        'printf RETURN'*) return 2 ;;
    esac
    return 0
}
fixture_body() {
    printf BEFORE >> "$HOME/steps"
    printf RETURN >> "$HOME/steps"
    printf AFTER >> "$HOME/steps"
}
trap fixture_debug DEBUG
"#);
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            shell.plain("false; true; true; printf SKIP > \"$HOME/skipped\"; printf RUN > \"$HOME/ran\"");
            assert!(
                !home.0.join("skipped").exists(),
                "{name}, integrated={integrated}: extdebug skip lost"
            );
            assert_eq!(fs::read_to_string(home.0.join("ran")).unwrap(), "RUN");
            shell.plain("fixture_body");
            assert_eq!(
                fs::read_to_string(home.0.join("steps")).unwrap(),
                "BEFORE",
                "{name}, integrated={integrated}: extdebug return lost"
            );
            // After the startup files, the bootstrap's first statement saves
            // the status and the trap; the next one removes the trap. The
            // user's trap also sees the bootstrap's final `: "$_"`, which
            // the documentation lists; nothing else of Huterm's.
            let internals = fs::read_to_string(home.0.join("internals"))
                .unwrap_or_default();
            assert!(
                internals
                    .lines()
                    .all(|line| line.starts_with("_huterm_bash_last=$_ ")),
                "{name}, integrated={integrated}: Huterm commands reached the user's DEBUG trap: {internals:?}"
            );
            assert_eq!(
                fs::read_to_string(home.0.join("commands")).unwrap(),
                "0:false\n1:true\n0:true\n",
                "{name}, integrated={integrated}"
            );
        }
    }
}

/// Texts that a startup option or hook change turns into visible breakage.
fn assert_no_shell_errors(name: &str, text: &str) {
    for error in [
        "unbound variable",
        "parameter not set",
        "command not found",
        "invalid option",
        "syntax error",
        "%{",
        "failed to load",
        "_huterm_",
        "\\073",
    ] {
        assert!(!text.contains(error), "{name}: {error:?} in {text:?}");
    }
}

/// Runs the first prompt, empty input, and failing and succeeding commands,
/// asserting one marker stream with no C or D for empty input.
fn assert_marked_session(name: &str, startup: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    let (home, mut command) = fixture(name, false);
    command.program = binary;
    let zsh = name == "zsh";
    home.write(
        if zsh { ".zshrc" } else { ".bashrc" },
        &format!(
            "{}{startup}",
            if zsh {
                "PROMPT='READY> '\n"
            } else {
                "PS1='READY> '\n"
            }
        ),
    );
    let mut shell = Shell::spawn(&command);
    shell.until(|bytes| markers(bytes).last().is_some_and(|mark| mark == "B"));
    let mut transcript =
        String::from_utf8_lossy(&shell.transcript).into_owned();
    assert_eq!(
        markers(&shell.transcript),
        ["A", "B"],
        "{name} {startup:?}: first prompt {transcript:?}"
    );
    for (input, status) in [("", ""), ("false", "1"), ("", ""), ("true", "0")] {
        if input.is_empty() {
            shell.transcript.clear();
            shell.send("\n");
            shell.until(|bytes| {
                markers(bytes).last().is_some_and(|mark| mark == "B")
            });
            assert_eq!(
                markers(&shell.transcript),
                ["A", "B"],
                "{name} {startup:?}: empty input {:?}",
                String::from_utf8_lossy(&shell.transcript)
            );
        } else {
            assert_stream(&mut shell, name, input, status);
        }
        transcript.push_str(&String::from_utf8_lossy(&shell.transcript));
    }
    assert_no_shell_errors(name, &transcript);
    eprintln!("native {name}: marker stream with {startup:?} ran");
}

#[test]
fn native_bash_strict_options_and_traced_traps_keep_one_stream() {
    for name in ["bash", "/bin/bash"] {
        for startup in [
            "set -u\n",
            "set -T\n",
            "shopt -s extdebug\n",
            "set -T\ntrap ': \"$BASH_COMMAND\"' DEBUG\n",
            "shopt -s extdebug\ntrap ': \"$BASH_COMMAND\"' DEBUG\n",
            "set -u\nshopt -s extdebug\nPROMPT_COMMAND='PS1=\"READY> \"'\n",
        ] {
            assert_marked_session(name, startup);
        }
    }
}

#[test]
fn native_bash_promptvars_off_keeps_one_stream() {
    for name in ["bash", "/bin/bash"] {
        for startup in [
            "shopt -u promptvars\n",
            "shopt -u promptvars\ntheme() { PS1='READY> '; }\nPROMPT_COMMAND=theme\n",
        ] {
            assert_marked_session(name, startup);
        }
    }
}

#[test]
fn native_zsh_strict_options_and_replaced_hook_arrays_keep_one_stream() {
    for startup in [
        "setopt nounset\n",
        "setopt nounset err_return\nfixture_prompt() { PROMPT='READY> '; }\nprecmd_functions+=(fixture_prompt)\n",
        // A startup file that assigns the hook arrays drops earlier hooks.
        "fixture_prompt() { PROMPT='READY> '; }\nprecmd_functions=(fixture_prompt)\npreexec_functions=()\nchpwd_functions=()\n",
    ] {
        assert_marked_session("zsh", startup);
    }
}

#[test]
fn native_zsh_without_prompt_percent_prints_a_and_omits_b() {
    let Some(binary) = executable("zsh") else {
        return;
    };
    let (home, mut command) = fixture("zsh", false);
    command.program = binary;
    home.write(".zshrc", "unsetopt prompt_percent\nPROMPT='READY> '\n");
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    assert_eq!(markers(&shell.transcript), ["A"]);
    for status in ["1", "0"] {
        shell.line(if status == "1" { "false" } else { "true" });
        assert_eq!(
            markers(&shell.transcript),
            ["C", &format!("D;{status}"), "A"],
            "{:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        assert_no_shell_errors(
            "zsh",
            &String::from_utf8_lossy(&shell.transcript),
        );
    }
}

/// Records what the next command line and the prompt hooks see after each
/// command, for a plain and an integrated start of the same shell.
fn shell_state_after_commands(
    name: &str,
    binary: &Path,
    integrated: bool,
    startup: &str,
    record: &str,
) -> (String, String, Vec<String>) {
    let (home, mut command) = fixture(name, false);
    binary.clone_into(&mut command.program);
    if !integrated {
        command.shell_integration = huterm_protocol::ShellIntegration::OFF;
    }
    home.write(if name == "zsh" { ".zshrc" } else { ".bashrc" }, startup);
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    let mut prompts = Vec::new();
    for input in ["false", "false | true", "echo SENT", "true SENT2"] {
        shell.line(input);
        let text = String::from_utf8_lossy(&shell.transcript).into_owned();
        let prompt = text.rsplit_once('\n').map_or(&*text, |(_, last)| last);
        let start = prompt.find("[[").expect("state prompt");
        prompts.push(prompt[start..prompt.find("]]").unwrap() + 2].to_owned());
        shell.line(record);
    }
    let read =
        |file: &str| fs::read_to_string(home.0.join(file)).unwrap_or_default();
    (read("next"), read("hooks"), prompts)
}

#[test]
fn native_zsh_preserves_status_pipeline_and_last_argument() {
    let Some(binary) = executable("zsh") else {
        return;
    };
    // A theme registered after Huterm's hooks rebuilds the prompt each time
    // and records what prompt hooks see.
    let startup = r#"skip_global_compinit=1
fixture_theme() {
    print -r -- "$?|$pipestatus|$_" >> "$HOME/hooks"
    PROMPT='[[%?]]READY> '
}
precmd_functions+=(fixture_theme)
"#;
    let record = r#"print -r -- "$?|$pipestatus|$_" >> "$HOME/next""#;
    let plain =
        shell_state_after_commands("zsh", &binary, false, startup, record);
    let integrated =
        shell_state_after_commands("zsh", &binary, true, startup, record);
    assert_eq!(
        plain.2,
        ["[[1]]", "[[0]]", "[[0]]", "[[0]]"],
        "plain prompt status"
    );
    assert_eq!(
        plain.0, "1|1|false\n0|1 0|true\n0|0|SENT\n0|0|SENT2\n",
        "plain baseline"
    );
    assert_eq!(integrated, plain, "zsh state after commands");
}

#[test]
fn native_bash_prompt_command_and_prompt_see_plain_state() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let apple = name == "/bin/bash" && cfg!(target_os = "macos");
        let array = prompt_command_arrays(&binary);
        for form in ["scalar", "array"] {
            if form == "array" && !array {
                continue;
            }
            // A constant last argument keeps $_ independent of PIPESTATUS.
            let entry = r#"printf "%s|%s|%s|%s\n" "$?" "$_" "${PIPESTATUS[*]}" end >> "$HOME/hooks""#;
            let startup = format!(
                "PS1='[[$?|${{PIPESTATUS[*]}}]]READY> '\nPROMPT_COMMAND={}\n",
                if form == "array" {
                    format!(
                        "('{entry}' 'fixture_theme')\nfixture_theme() {{ :; }}"
                    )
                } else {
                    format!("'{entry}'")
                }
            );
            let record = r#"printf '%s|%s|%s|%s\n' "$?" "$_" "${PIPESTATUS[*]}" end >> "$HOME/next""#;
            let plain = shell_state_after_commands(
                name, &binary, false, &startup, record,
            );
            let integrated = shell_state_after_commands(
                name, &binary, true, &startup, record,
            );
            assert!(
                plain.1.contains("|1 0|end")
                    && plain.1.contains("0|SENT|0|end"),
                "{name} {form}: plain baseline {:?}",
                plain.1
            );
            // The first prompt's $_ comes from the bootstrap sourcing the
            // startup file, so compare only its status.
            let first = |text: &str| {
                text.lines()
                    .next()
                    .unwrap()
                    .split('|')
                    .next()
                    .unwrap()
                    .to_owned()
            };
            assert_eq!(
                first(&integrated.1),
                first(&plain.1),
                "{name} {form}: first prompt status"
            );
            let later = |text: &str| {
                text.lines().skip(1).collect::<Vec<_>>().join("\n")
            };
            let (mut plain, mut integrated) = (
                (plain.0, later(&plain.1), plain.2),
                (integrated.0, later(&integrated.1), integrated.2),
            );
            if apple {
                // Jim accepted on 2026-11-04 that any DEBUG trap on Apple Bash
                // 3.2 leaves only the last status in PIPESTATUS. $? and $_
                // still match a plain start.
                assert!(
                    !integrated.1.contains("|1 0|"),
                    "{name}: {:?}",
                    integrated.1
                );
                let status_and_argument = |text: &str| -> String {
                    text.lines()
                        .map(|line| {
                            line.splitn(3, '|')
                                .take(2)
                                .collect::<Vec<_>>()
                                .join("|")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                for state in [&mut plain, &mut integrated] {
                    state.0 = status_and_argument(&state.0);
                    state.1 = status_and_argument(&state.1);
                    for prompt in &mut state.2 {
                        *prompt = prompt.split('|').next().unwrap().to_owned();
                    }
                }
            }
            assert_eq!(
                integrated, plain,
                "{name} {form}: state after commands"
            );
        }
    }
}

#[test]
fn native_bash_prompt_command_installers_keep_their_entries() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        // A first-prompt installer that replaces PROMPT_COMMAND with a theme,
        // or appends a command to it, in the scalar and the array form.
        let mut cases = vec![
            (
                "PROMPT_COMMAND=fixture_lazy",
                "PROMPT_COMMAND=fixture_theme",
            ),
            (
                "PROMPT_COMMAND=fixture_lazy",
                "PROMPT_COMMAND+='; fixture_after'",
            ),
            (
                "PROMPT_COMMAND=fixture_lazy",
                "PROMPT_COMMAND+='fixture_after'",
            ),
            (
                "PROMPT_COMMAND=fixture_lazy",
                "PROMPT_COMMAND=\"fixture_after;$PROMPT_COMMAND\"",
            ),
        ];
        if prompt_command_arrays(&binary) {
            cases.extend([
                (
                    "PROMPT_COMMAND=(fixture_lazy)",
                    "PROMPT_COMMAND=(fixture_theme)",
                ),
                (
                    "PROMPT_COMMAND=(fixture_lazy)",
                    "PROMPT_COMMAND+=(fixture_after)",
                ),
                (
                    "PROMPT_COMMAND=(fixture_lazy)",
                    "PROMPT_COMMAND+='; fixture_after'",
                ),
                (
                    "PROMPT_COMMAND=(fixture_lazy)",
                    "PROMPT_COMMAND=fixture_theme",
                ),
            ]);
        }
        for (initial, install) in cases {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            home.write(
                ".bashrc",
                &format!(
                    r#"PS1='READY> '
fixture_theme() {{ PS1='READY> '; printf theme >> "$HOME/theme"; }}
fixture_after() {{ printf after >> "$HOME/after"; }}
fixture_lazy() {{
    [[ -z ${{fixture_once-}} ]] || return 0
    fixture_once=1
    {install}
}}
{initial}
"#
                ),
            );
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            let mut transcript =
                String::from_utf8_lossy(&shell.transcript).into_owned();
            for (input, status) in [("false", "1"), ("true", "0")] {
                assert_stream(&mut shell, name, input, status);
                transcript
                    .push_str(&String::from_utf8_lossy(&shell.transcript));
            }
            assert_no_shell_errors(name, &transcript);
            let ran = |file: &str| {
                fs::read_to_string(home.0.join(file)).unwrap_or_default()
            };
            let expected = if install.contains("fixture_theme") {
                "theme"
            } else {
                "after"
            };
            assert!(
                ran(expected).len() >= 2 * expected.len(),
                "{name} {initial} {install}: {expected} did not keep running"
            );
            eprintln!(
                "native {name}: installer {initial} then {install:?} ran"
            );
        }
    }
}

#[test]
fn native_bash_preexec_trap_never_sees_huterm_entries() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        // A preexec-style trap armed by its prompt hook, as prompt themes
        // without bash-preexec use to time the next command.
        let startup = r#"PS1='READY> '
fixture_precmd() { fixture_ready=1; }
PROMPT_COMMAND=fixture_precmd
fixture_preexec() {
    if [[ ${fixture_ready-} == 1 ]]; then
        fixture_ready=0
        printf '%s\n' "$BASH_COMMAND" >> "$HOME/started"
    fi
}
trap 'fixture_preexec' DEBUG
"#;
        let mut logs = Vec::new();
        for integrated in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            if !integrated {
                command.shell_integration =
                    huterm_protocol::ShellIntegration::OFF;
            }
            home.write(".bashrc", startup);
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            shell.line("true one");
            shell.line("true two");
            logs.push(fs::read_to_string(home.0.join("started")).unwrap());
        }
        assert_eq!(logs[0], "true one\ntrue two\n", "{name}: plain baseline");
        assert_eq!(logs[1], logs[0], "{name}: trap saw Huterm's commands");
    }
}

#[test]
fn native_bash_options_and_exported_functions_match_plain_start() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for login in [false, true] {
            let mut states = Vec::new();
            for integrated in [false, true] {
                let (home, mut command) = fixture(name, login);
                command.program.clone_from(&binary);
                if !integrated {
                    command.shell_integration =
                        huterm_protocol::ShellIntegration::OFF;
                }
                // POSIX mode refuses to import a function whose name is not
                // a POSIX name.
                command.environment.push((
                    "BASH_FUNC_fixture-greet%%".into(),
                    "() {  printf GREET; }".into(),
                ));
                let mut shell = Shell::spawn(&command);
                shell.prompt();
                assert_no_shell_errors(
                    name,
                    &String::from_utf8_lossy(&shell.transcript),
                );
                assert!(
                    !String::from_utf8_lossy(&shell.transcript)
                        .contains("error importing"),
                    "{name} login={login} integrated={integrated}: {:?}",
                    String::from_utf8_lossy(&shell.transcript)
                );
                shell.plain(
                    "{ shopt; set -o; fixture-greet; } > \"$HOME/state\"",
                );
                let state = fs::read_to_string(home.0.join("state")).unwrap();
                // Jim accepted on 2026-11-04 that an emulated login shell
                // reports login_shell off.
                states.push(
                    state
                        .lines()
                        .filter(|line| !line.starts_with("login_shell"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            assert!(states[0].ends_with("GREET"), "{name}: {}", states[0]);
            assert_eq!(
                states[1], states[0],
                "{name} login={login}: options or functions differ"
            );
            eprintln!(
                "native {name}: options match a plain start (login={login})"
            );
        }
    }
}

#[test]
fn native_bash_exported_prompts_stay_inert_in_nested_shells() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        for nested_rc in [true, false] {
            let mut records = Vec::new();
            for integrated in [false, true] {
                let (home, mut command) = fixture(name, false);
                command.program.clone_from(&binary);
                if !integrated {
                    command.shell_integration =
                        huterm_protocol::ShellIntegration::OFF;
                }
                // The fixture environment already exports PS1.
                home.write(
                    ".bashrc",
                    "export PS1='READY> '\nexport PROMPT_COMMAND=\"history -a; $PROMPT_COMMAND\"\n",
                );
                let mut shell = Shell::spawn(&command);
                shell.prompt();
                if integrated {
                    assert_stream(&mut shell, name, "false", "1");
                }
                shell.transcript.clear();
                shell.send(&format!(
                    "'{}' {}-i\n",
                    binary.display(),
                    if nested_rc { "" } else { "--norc " }
                ));
                shell.until(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .split_once("-i\r\n")
                        .is_some_and(|(_, rest)| rest.contains("READY>"))
                });
                let mut nested =
                    String::from_utf8_lossy(&shell.transcript).into_owned();
                // A user entry after the inherited ones sees what Huterm's
                // inherited entries leave behind.
                let mut lines = vec![
                    r#"PROMPT_COMMAND+=$'\n''printf "%s|%s\n" "$?" "$_" >> "$HOME/nested-hooks"'"#,
                ];
                for input in ["false", "true SENT", "(exit 3)", "true LAST"] {
                    lines.push(input);
                    lines.push(
                        r#"printf '%s|%s\n' "$?" "$_" >> "$HOME/nested-next""#,
                    );
                }
                for line in lines {
                    shell.line(line);
                    nested
                        .push_str(&String::from_utf8_lossy(&shell.transcript));
                }
                assert_no_shell_errors(name, &nested);
                // The parent marks the command that started the child.
                if integrated {
                    nested = nested.replacen("\x1b]133;C\x07", "", 1);
                }
                assert!(
                    !nested.contains("]133;"),
                    "{name} nested_rc={nested_rc} integrated={integrated}: {nested:?}"
                );
                shell.line("exit");
                records.push((
                    fs::read_to_string(home.0.join("nested-hooks")).unwrap(),
                    fs::read_to_string(home.0.join("nested-next")).unwrap(),
                ));
                if integrated {
                    // The parent keeps its own stream after the child exits.
                    assert_stream(&mut shell, name, "true", "0");
                }
            }
            let statuses = records[0]
                .1
                .lines()
                .map(|line| line.split('|').next().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                statuses,
                ["1", "0", "3", "0"],
                "{name}: plain baseline"
            );
            assert_eq!(
                records[1], records[0],
                "{name} nested_rc={nested_rc}: nested state differs"
            );
            eprintln!(
                "native {name}: nested shell with exported prompts ran (rc={nested_rc})"
            );
        }
    }
}

#[test]
fn native_bash_resourced_startup_file_fails_cleanly() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        // Re-sourcing either replaces PROMPT_COMMAND, which removes Huterm's
        // hook, or keeps its entries; both assign a new PS1.
        for keeps_entries in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            home.write(
                ".bashrc",
                &format!(
                    "fixture_theme() {{ :; }}\nPS1=\"${{FIXTURE_PS1:-READY> }}\"\nPROMPT_COMMAND={}\n",
                    if keeps_entries {
                        "\"fixture_theme${PROMPT_COMMAND:+$'\\n'$PROMPT_COMMAND}\""
                    } else {
                        "fixture_theme"
                    }
                ),
            );
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            assert_stream(&mut shell, name, "false", "1");
            shell.line("FIXTURE_PS1='AGAIN READY> '; source \"$HOME/.bashrc\"");
            let mut later = String::new();
            for (input, status) in [("false", "1"), ("true", "0")] {
                if keeps_entries {
                    assert_stream(&mut shell, name, input, status);
                } else {
                    shell.line(input);
                    assert_eq!(
                        markers(&shell.transcript),
                        [] as [String; 0],
                        "{name}: a mark after Huterm's hook was replaced: {:?}",
                        String::from_utf8_lossy(&shell.transcript)
                    );
                }
                later.push_str(&String::from_utf8_lossy(&shell.transcript));
            }
            assert!(
                later.contains("AGAIN READY> "),
                "{name} keeps_entries={keeps_entries}: new PS1 not shown: {later:?}"
            );
            assert_no_shell_errors(name, &later);
            eprintln!(
                "native {name}: re-sourced startup file (keeps_entries={keeps_entries})"
            );
        }
    }
}

#[test]
fn native_bash_uninstalled_locale_adds_no_warnings() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let mut counts = Vec::new();
        for integrated in [false, true] {
            let (home, mut command) = fixture(name, false);
            command.program.clone_from(&binary);
            if !integrated {
                command.shell_integration =
                    huterm_protocol::ShellIntegration::OFF;
            }
            command
                .environment
                .push(("LC_ALL".into(), "xx_XX.UTF-8".into()));
            for control in ["\u{1}", "\u{7f}"] {
                fs::create_dir(home.0.join(format!("control{control}")))
                    .unwrap();
            }
            let mut shell = Shell::spawn(&command);
            shell.prompt();
            let mut transcript =
                String::from_utf8_lossy(&shell.transcript).into_owned();
            for input in [
                "true",
                "cd \"$HOME/control$(/usr/bin/printf '\\001')\"",
                "cd \"$HOME/control$(/usr/bin/printf '\\177')\"",
                "cd \"$HOME\"",
            ] {
                shell.line(input);
                let text = String::from_utf8_lossy(&shell.transcript);
                if integrated && input.contains("control") {
                    assert!(
                        directory_reports(&shell.transcript).is_empty(),
                        "{name}: reported a control path: {text:?}"
                    );
                }
                transcript.push_str(&text);
            }
            counts.push(transcript.matches("setlocale").count());
        }
        assert_eq!(counts[1], counts[0], "{name}: locale warnings");
        eprintln!("native {name}: uninstalled locale warnings {counts:?}");
    }
}

#[test]
fn native_zsh_framework_marks_survive_and_suppress_huterm_marks() {
    let Some(binary) = executable("zsh") else {
        return;
    };
    for terminator in [r"\e\\", r"\a"] {
        let (home, mut command) = fixture("zsh", false);
        command.program.clone_from(&binary);
        home.write(
            ".zshrc",
            &format!(
                r#"skip_global_compinit=1
fixture_running=0
PROMPT=$'%{{\e]133;A{terminator}%}}READY> %{{\e]133;B{terminator}%}}'
fixture_preexec() {{ printf '\e]133;C{terminator}'; fixture_running=1; }}
fixture_precmd() {{
    local code=$?
    if (( fixture_running )); then printf '\e]133;D;%s{terminator}' "$code"; fi
    fixture_running=0
}}
preexec_functions+=(fixture_preexec)
precmd_functions+=(fixture_precmd)
"#
            ),
        );
        let mut shell = Shell::spawn(&command);
        shell.until(|bytes| markers(bytes).last().is_some_and(|m| m == "B"));
        assert_eq!(
            markers(&shell.transcript),
            ["A", "B"],
            "{terminator}: {:?}",
            String::from_utf8_lossy(&shell.transcript)
        );
        for (input, status) in [("false", "1"), ("true", "0"), ("false", "1")] {
            shell.line(input);
            assert_eq!(
                markers(&shell.transcript),
                ["C", &format!("D;{status}"), "A", "B"],
                "{terminator}: {:?}",
                String::from_utf8_lossy(&shell.transcript)
            );
        }
        assert!(
            String::from_utf8_lossy(&shell.transcript)
                .contains("]7;kitty-shell-cwd://"),
            "{terminator}: OSC 7 stays active"
        );
    }
}

#[test]
fn native_bash_keeps_a_pending_preexec_installer_last() {
    for name in ["bash", "/bin/bash"] {
        let Some(binary) = executable(name) else {
            continue;
        };
        let (home, mut command) = fixture(name, false);
        command.program = binary;
        // bash-preexec 0.7 appends this installer line, then replaces it with
        // ":" and sanitizes PROMPT_COMMAND, which takes seconds before Bash
        // 5.1 when the ":" is not on the last line. The sanitizing removes a
        // final ":".
        home.write(
            ".bashrc",
            r#"PS1='READY> '
fixture_theme() { :; }
__bp_install() {
    printf '%s\n' "${PROMPT_COMMAND##*$'\n'}" > "$HOME/installer"
    PROMPT_COMMAND=${PROMPT_COMMAND//"__bp_install \"\$_\""/:}
    PROMPT_COMMAND=${PROMPT_COMMAND%$'\n:'}
}
PROMPT_COMMAND=$'fixture_theme\n__bp_install "$_"'
"#,
        );
        let mut shell = Shell::spawn(&command);
        shell.prompt();
        assert_eq!(
            fs::read_to_string(home.0.join("installer")).unwrap(),
            "__bp_install \"$_\"\n",
            "{name}: Huterm's entry followed the pending installer"
        );
        assert_stream(&mut shell, name, "false", "1");
    }
}
