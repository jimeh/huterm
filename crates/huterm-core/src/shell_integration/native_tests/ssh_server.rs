//! `ssh` from an integrated shell to a real `sshd` on loopback.
//!
//! A non-root `sshd` can log in only its own user, so these tests run only
//! where that account's home is disposable: the Linux Docker container and
//! CI runners. `HUTERM_SSH_TESTS=1` enables them, and they never run on
//! macOS, even with it set. With it set, a missing `sshd`, `sftp-server`,
//! zsh-autosuggestions, or `huterm` executable is a failure. Build `huterm`
//! first (`cargo build -p huterm-cli`); `cargo test --workspace` does.
//!
//! The server's `ForceCommand` runs a given command with `sh -c` and starts
//! `zsh -il` otherwise, in a scratch home where zsh-autosuggestions is on.
//! The client authenticates with a scratch key only, so the server log's
//! `Accepted publickey` lines count authentications.

use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::process::{Child, Command, Stdio};

use huterm_protocol::ShellFeature;

use super::*;

const AUTOSUGGESTIONS: &str =
    "/usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh";

fn enabled() -> bool {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping sshd tests: they run only on Linux");
        return false;
    }
    if std::env::var_os("HUTERM_SSH_TESTS").as_deref() != Some(OsStr::new("1"))
    {
        eprintln!("skipping sshd tests: set HUTERM_SSH_TESTS=1 to run them");
        return false;
    }
    true
}

fn executable_file(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn required(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    assert!(
        path.exists(),
        "HUTERM_SSH_TESTS=1 requires {}",
        path.display()
    );
    path
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{command:?}: {output:?}");
}

/// A scratch `sshd` and the files a client needs to reach it.
struct Server {
    _root: Directory,
    process: Child,
    remote_home: PathBuf,
    client_config: PathBuf,
    log: PathBuf,
}

impl Server {
    fn start() -> Self {
        let sshd = required("/usr/sbin/sshd");
        let sftp = required("/usr/lib/openssh/sftp-server");
        required(AUTOSUGGESTIONS);
        let root = Directory::new();
        let path = |name: &str| root.0.join(name);
        for directory in ["server", "client", "remote", "no-tic-bin"] {
            fs::create_dir_all(path(directory)).unwrap();
        }
        for key in ["server/host_key", "client/id"] {
            run(Command::new("ssh-keygen")
                .args([
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    "huterm-test",
                    "-f",
                ])
                .arg(path(key)));
        }
        fs::copy(path("client/id.pub"), path("server/authorized_keys"))
            .unwrap();
        let remote_home = path("remote");
        write_remote_home(&remote_home);
        // A remote without tic: only what the install script starts with.
        for tool in ["sh", "cat", "mktemp", "rm", "mkdir"] {
            let file = executable(tool).unwrap();
            symlink(file, path("no-tic-bin").join(tool)).unwrap();
        }
        let force = path("server/force.sh");
        executable_file(
            &force,
            &format!(
                "#!/bin/sh\ncd \"$HOME\" || exit 1\n\
                 if [ -n \"$SSH_ORIGINAL_COMMAND\" ]; then\n\
                 \x20   if [ -e \"$HOME/no-tic\" ]; then PATH={}; export PATH; fi\n\
                 \x20   exec sh -c \"$SSH_ORIGINAL_COMMAND\"\n\
                 fi\nexec zsh -il\n",
                path("no-tic-bin").display()
            ),
        );
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = path("server/sshd_config");
        write_sshd_config(&root.0, port, &sftp);
        let log = path("server/log");
        let process = Command::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let user = nix::unistd::User::from_uid(nix::unistd::geteuid())
            .unwrap()
            .unwrap()
            .name;
        let client_config = path("client/config");
        fs::write(
            &client_config,
            format!(
                "Host test\n    HostName 127.0.0.1\n    Port {port}\n    User {user}\n\
                 \x20   IdentityFile {id}\n    IdentitiesOnly yes\n    BatchMode yes\n\
                 \x20   UserKnownHostsFile {known}\n    GlobalKnownHostsFile /dev/null\n\
                 \x20   StrictHostKeyChecking accept-new\n",
                id = path("client/id").display(),
                known = path("client/known_hosts").display(),
            ),
        )
        .unwrap();
        let server = Self {
            _root: root,
            process,
            remote_home,
            client_config,
            log,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server.log_text().contains("Server listening on 127.0.0.1") {
            assert!(
                Instant::now() < deadline,
                "sshd did not start: {}",
                server.log_text()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn authentications(&self) -> usize {
        self.log_text().matches("Accepted publickey").count()
    }

    fn remote_entry(&self) -> bool {
        ["x", "78"].iter().any(|layout| {
            self.remote_home
                .join(".terminfo")
                .join(layout)
                .join("xterm-huterm")
                .is_file()
        })
    }

    fn ssh(&self) -> String {
        format!("-F '{}' test", self.client_config.display())
    }
}

/// The server's configuration in `<root>/server`, for the remote home in
/// `<root>/remote`.
fn write_sshd_config(root: &Path, port: u16, sftp: &Path) {
    let path = |name: &str| root.join(name);
    fs::write(
        path("server/sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\n\
             PidFile {pid}\nAuthorizedKeysFile {keys}\nUsePAM no\n\
             PermitUserRC no\nStrictModes no\nLogLevel VERBOSE\n\
             PubkeyAuthentication yes\nPasswordAuthentication no\n\
             KbdInteractiveAuthentication no\n\
             SetEnv HOME={home}\n\
             AcceptEnv COLORTERM TERM_PROGRAM TERM_PROGRAM_VERSION\n\
             Subsystem sftp {sftp}\nForceCommand {force}\n",
            host = path("server/host_key").display(),
            pid = path("server/sshd.pid").display(),
            keys = path("server/authorized_keys").display(),
            home = path("remote").display(),
            sftp = sftp.display(),
            force = path("server/force.sh").display(),
        ),
    )
    .unwrap();
}

/// A zsh home whose history suggests `echo suggested-text`.
fn write_remote_home(home: &Path) {
    fs::write(home.join(".zshenv"), "skip_global_compinit=1\n").unwrap();
    fs::write(
        home.join(".zshrc"),
        format!(
            "PROMPT='REMOTE> '\nRPROMPT=''\nHISTFILE=$HOME/.zsh_history\n\
             HISTSIZE=100\nSAVEHIST=100\nsource {AUTOSUGGESTIONS}\n"
        ),
    )
    .unwrap();
    fs::write(home.join(".zsh_history"), "echo suggested-text\n").unwrap();
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// A local integrated zsh whose `TERM` is `xterm-huterm`, with the built
/// `huterm` in its `HUTERM_BIN_DIR`, a scratch state root, and `shims`
/// first on `PATH`.
fn local_shell(features: &[ShellFeature]) -> (Shell, Directory) {
    let huterm = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("huterm");
    assert!(
        huterm.is_file(),
        "HUTERM_SSH_TESTS=1 requires {}; build it with cargo build -p huterm-cli",
        huterm.display()
    );
    let binary = executable("zsh").expect("HUTERM_SSH_TESTS=1 requires zsh");
    let (home, mut command) = fixture("zsh", false);
    command.program = binary;
    command.shell_integration.features = features.iter().copied().collect();
    for directory in ["bin", "shims", "terminfo", "state", "run"] {
        fs::create_dir_all(home.0.join(directory)).unwrap();
    }
    symlink(&huterm, home.0.join("bin/huterm")).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/terminfo/xterm-huterm.terminfo");
    run(Command::new("tic")
        .args(["-x", "-o"])
        .arg(home.0.join("terminfo"))
        .arg(source));
    command.environment.extend([
        ("TERM".into(), "xterm-huterm".into()),
        (
            "TERMINFO_DIRS".into(),
            format!("{}:", home.0.join("terminfo").display()),
        ),
        ("COLORTERM".into(), "truecolor".into()),
        (
            "HUTERM_STATE_DIR".into(),
            home.0.join("state").display().to_string(),
        ),
        (
            "XDG_RUNTIME_DIR".into(),
            home.0.join("run").display().to_string(),
        ),
        (
            "PATH".into(),
            format!(
                "{}:{}",
                home.0.join("shims").display(),
                std::env::var("PATH").unwrap()
            ),
        ),
    ]);
    let process = crate::pty::spawn_with_plan_isolated(
        &command,
        launch_plan_in(&command, None, home.0.join("bin")),
    )
    .unwrap();
    let mut shell = Shell::from_process(&command, process);
    shell.prompt();
    (shell, home)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Types `input` at the local prompt and waits for the remote prompt.
#[track_caller]
fn connect(shell: &mut Shell, input: &str) {
    shell.transcript.clear();
    shell.send(&format!("{input}\n"));
    shell.until(|bytes| text(bytes).contains("REMOTE> "));
}

/// Runs `command` in the remote zsh and returns the line it prints after
/// `marker`.
#[track_caller]
fn remote_line(shell: &mut Shell, command: &str, marker: &str) -> String {
    shell.transcript.clear();
    shell.send(&format!("{command}\n"));
    shell.until(|bytes| {
        text(bytes)
            .rsplit_once(&format!("\n{marker}"))
            .is_some_and(|(_, rest)| rest.contains('\n'))
    });
    let transcript = text(&shell.transcript);
    let (_, rest) = transcript.rsplit_once(&format!("\n{marker}")).unwrap();
    rest.lines().next().unwrap().trim_end().to_owned()
}

/// Clears the remote line, leaves the remote zsh, and waits for the local
/// prompt. Ctrl-U clears the line without a signal, which would discard
/// typeahead, and without adding the line to the history the next
/// suggestion comes from.
#[track_caller]
fn disconnect(shell: &mut Shell) {
    shell.transcript.clear();
    shell.send("\x15exit\n");
    shell.until(|bytes| text(bytes).contains("READY> "));
}

/// The cursor row's text, and the cursor column.
fn cursor_line(shell: &mut Shell) -> (String, Option<u16>) {
    let snapshot = shell.engine.snapshot().unwrap();
    let Some(cursor) = snapshot.cursor else {
        return (String::new(), None);
    };
    let row = &snapshot.rows[usize::from(cursor.row)];
    let line: String =
        row.cells.iter().map(|cell| cell.text.displayed()).collect();
    (line.trim_end().to_owned(), Some(cursor.column))
}

/// Polls the screen until the cursor row shows `line` with the cursor at
/// `column`, and reports the last state on timeout.
fn screen_shows(
    shell: &mut Shell,
    line: &str,
    column: u16,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (shown, at) = cursor_line(shell);
        if shown == line && at == Some(column) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "expected {line:?} with the cursor at column {column}; \
                 the cursor row shows {shown:?} with the cursor at {at:?}"
            ));
        }
        shell.pump(deadline);
    }
}

/// Types a suggested command, accepts the suggestion, and edits it, checking
/// the rendered line and cursor after each step.
fn line_editor_works(shell: &mut Shell) -> Result<(), String> {
    for (input, line, column) in [
        ("echo sugg", "REMOTE> echo suggested-text", 17),
        // Ctrl-E accepts zsh-autosuggestions' suggestion.
        ("\x05", "REMOTE> echo suggested-text", 27),
        ("\x7f\x7fXY", "REMOTE> echo suggested-teXY", 27),
    ] {
        shell.send(input);
        screen_shows(shell, line, column)
            .map_err(|error| format!("after {input:?}: {error}"))?;
    }
    Ok(())
}

#[test]
fn native_ssh_fallback_keeps_a_remote_line_editor_working() {
    if !enabled() {
        return;
    }
    let server = Server::start();
    let (mut shell, _home) = local_shell(&[
        ShellFeature::Path,
        ShellFeature::SshEnv,
        ShellFeature::Sudo,
    ]);

    connect(&mut shell, &format!("ssh {}", server.ssh()));
    assert_eq!(
        remote_line(&mut shell, "echo \"T=$TERM\"", "T="),
        "xterm-256color"
    );
    line_editor_works(&mut shell).unwrap();
    disconnect(&mut shell);

    // The same check fails over plain ssh, which sends xterm-huterm to a
    // host without the entry.
    connect(&mut shell, &format!("command ssh {}", server.ssh()));
    let control = line_editor_works(&mut shell);
    assert!(control.is_err(), "the check passed without the fallback");
    eprintln!(
        "control run failed the check as expected: {}",
        control.unwrap_err()
    );
    disconnect(&mut shell);
    eprintln!("native ssh fallback: line editor checks ran");
}

#[test]
fn native_ssh_install_authenticates_once_and_falls_back() {
    if !enabled() {
        return;
    }
    let server = Server::start();
    let all = [
        ShellFeature::Path,
        ShellFeature::SshEnv,
        ShellFeature::SshTerminfo,
        ShellFeature::Sudo,
    ];
    let (mut shell, home) = local_shell(&all);
    let ssh = format!("ssh {}", server.ssh());

    // A new host: one authentication covers the install and the session.
    connect(&mut shell, &ssh);
    assert!(
        text(&shell.transcript)
            .contains("installing the xterm-huterm terminfo entry"),
        "{}",
        text(&shell.transcript)
    );
    assert_eq!(
        remote_line(&mut shell, "echo \"T=$TERM P=$TERM_PROGRAM\"", "T="),
        "xterm-huterm P=Huterm"
    );
    assert!(server.remote_entry());
    assert_eq!(server.authentications(), 1, "{}", server.log_text());
    disconnect(&mut shell);
    let cache = fs::read_to_string(home.0.join("state/ssh-cache")).unwrap();
    assert!(cache.contains("@127.0.0.1\t"), "{cache}");

    // A cached host connects without an install.
    connect(&mut shell, &ssh);
    assert!(!text(&shell.transcript).contains("installing"));
    assert_eq!(
        remote_line(&mut shell, "echo \"T=$TERM\"", "T="),
        "xterm-huterm"
    );
    assert_eq!(server.authentications(), 2);
    disconnect(&mut shell);

    // A remote without tic falls back.
    let reset = |shell: &mut Shell| {
        shell.plain("\"$HUTERM_BIN_DIR/huterm\" ssh-cache clear");
        let _ = fs::remove_dir_all(server.remote_home.join(".terminfo"));
    };
    reset(&mut shell);
    fs::write(server.remote_home.join("no-tic"), "").unwrap();
    connect(&mut shell, &ssh);
    assert!(
        text(&shell.transcript)
            .contains("could not install the xterm-huterm terminfo entry"),
        "{}",
        text(&shell.transcript)
    );
    assert_eq!(
        remote_line(&mut shell, "echo \"T=$TERM\"", "T="),
        "xterm-256color"
    );
    disconnect(&mut shell);
    fs::remove_file(server.remote_home.join("no-tic")).unwrap();

    // When the master is gone before the session reuses it, the session
    // connects on its own. The shim removes the socket at that moment.
    reset(&mut shell);
    executable_file(
        &home.0.join("shims/ssh"),
        "#!/bin/sh\nfor argument in \"$@\"; do\n    case $argument in\n\
         \x20       ControlMaster=no) reuse=1 ;;\n\
         \x20       ControlPath=*) socket=${argument#ControlPath=} ;;\n    esac\ndone\n\
         if [ -n \"$reuse\" ]; then rm -f \"$socket\"; echo huterm-test-master-removed >&2; fi\n\
         exec /usr/bin/ssh \"$@\"\n",
    );
    let before = server.authentications();
    connect(&mut shell, &ssh);
    assert!(text(&shell.transcript).contains("huterm-test-master-removed"));
    assert_eq!(
        remote_line(&mut shell, "echo \"T=$TERM\"", "T="),
        "xterm-huterm"
    );
    assert_eq!(
        server.authentications(),
        before + 2,
        "a separate session connection"
    );
    disconnect(&mut shell);
    fs::remove_file(home.0.join("shims/ssh")).unwrap();
    eprintln!(
        "native ssh install: one authentication, cache, and fallbacks ran"
    );
}

#[test]
fn native_ssh_file_transfers_and_git_are_unwrapped() {
    if !enabled() {
        return;
    }
    let server = Server::start();
    let all = [
        ShellFeature::Path,
        ShellFeature::SshEnv,
        ShellFeature::SshTerminfo,
        ShellFeature::Sudo,
    ];
    let (mut shell, home) = local_shell(&all);
    let config = server.client_config.display().to_string();
    let remote = server.remote_home.display().to_string();

    home.write("payload", "copied by scp\n");
    shell.plain(&format!(
        "scp -F '{config}' \"$HOME/payload\" 'test:{remote}/copied' && echo HUTERM_SCP_OK"
    ));
    assert!(
        text(&shell.transcript).contains("\nHUTERM_SCP_OK"),
        "{}",
        text(&shell.transcript)
    );
    assert_eq!(
        fs::read_to_string(server.remote_home.join("copied")).unwrap(),
        "copied by scp\n"
    );

    home.write("batch", &format!("ls {remote}\n"));
    shell.plain(&format!(
        "sftp -F '{config}' -b \"$HOME/batch\" test && echo HUTERM_SFTP_OK"
    ));
    let transcript = text(&shell.transcript);
    assert!(
        transcript.contains("copied")
            && transcript.contains("\nHUTERM_SFTP_OK"),
        "{transcript}"
    );

    let repository = server.remote_home.join("repo.git");
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .arg(format!("--git-dir={}", repository.display()))
            .args(arguments)
            .env("HOME", &home.0)
            .env("GIT_AUTHOR_NAME", "Huterm")
            .env("GIT_AUTHOR_EMAIL", "test@huterm.invalid")
            .env("GIT_COMMITTER_NAME", "Huterm")
            .env("GIT_COMMITTER_EMAIL", "test@huterm.invalid")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    run(Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&repository)
        .env("HOME", &home.0));
    let tree = git(&["mktree"]);
    let commit = git(&["commit-tree", &tree, "-m", "initial"]);
    git(&["update-ref", "refs/heads/main", &commit]);
    shell.plain(&format!(
        "GIT_SSH_COMMAND=\"ssh -F '{config}'\" git ls-remote 'test:{}'",
        repository.display()
    ));
    assert!(
        text(&shell.transcript).contains(&format!("{commit}\trefs/heads/main")),
        "{}",
        text(&shell.transcript)
    );
    eprintln!("native ssh: scp, sftp, and git ls-remote ran");
}
