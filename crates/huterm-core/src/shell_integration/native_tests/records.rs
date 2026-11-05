//! Command records that real shells' OSC 133 markers produce, read from the
//! engine that parsed their output. Shell availability follows
//! `HUTERM_REQUIRE_SHELLS` through [`executable`].

use huterm_protocol::{CommandRecord, CommandState};

use super::*;

/// A command line in `name`'s syntax, and the exit code it reports.
struct Case {
    label: &'static str,
    input: String,
    exit_code: i32,
}

fn cases(name: &str) -> Vec<Case> {
    let external = |command: &str| {
        if name == "nu" {
            format!("^{command}")
        } else {
            command.to_owned()
        }
    };
    let multiline = match name {
        "fish" => "begin\nfalse\nend".to_owned(),
        "nu" => "do {\n^false\n}".to_owned(),
        "elvish" => "if $true {\nfalse\n}".to_owned(),
        _ => "if true; then\nfalse\nfi".to_owned(),
    };
    vec![
        Case {
            label: "normal",
            input: external("true"),
            exit_code: 0,
        },
        Case {
            label: "failing",
            input: external("false"),
            exit_code: 1,
        },
        Case {
            label: "multiline",
            input: multiline,
            exit_code: 1,
        },
        Case {
            label: "full-screen",
            input: external(
                "/usr/bin/printf '\\033[?1049hFULL SCREEN\\033[?1049l'",
            ),
            exit_code: 0,
        },
    ]
}

fn newest(shell: &Shell) -> Option<CommandRecord> {
    shell.engine.commands().last().cloned()
}

fn run_records(name: &str) {
    let Some(binary) = executable(name) else {
        return;
    };
    let (home, mut command) = fixture(name, false);
    command.program.clone_from(&binary);
    let mut shell = Shell::spawn(&command);
    shell.prompt();
    let mut sequence = newest(&shell).map_or(0, |record| record.sequence);
    for case in cases(name) {
        shell.cycle(&case.input);
        let records = shell.engine.commands();
        let record = records.last().unwrap();
        assert_eq!(
            (record.sequence, record.state),
            (
                sequence + 1,
                CommandState::Finished {
                    exit_code: Some(case.exit_code)
                }
            ),
            "{name} {}: {records:?} after {:?}",
            case.label,
            String::from_utf8_lossy(&shell.transcript)
        );
        assert!(record.duration.is_some(), "{name} {}", case.label);
        // Fish alone sends the command line, without its control characters.
        let text = (name == "fish").then(|| case.input.replace('\n', ""));
        assert_eq!(record.text, text, "{name} {}", case.label);
        sequence = record.sequence;
    }

    // A redrawn prompt resends A but starts no command. Elvish's Ctrl-L
    // redraws no marked prompt, so it gets an empty line instead.
    shell.transcript.clear();
    shell.send(if name == "elvish" { "\n" } else { "\x0c" });
    shell.until(|bytes| {
        markers(bytes).iter().any(|mark| mark == "A")
            && String::from_utf8_lossy(bytes).contains("READY>")
    });
    shell.cycle(&cases(name)[0].input);
    assert_eq!(
        newest(&shell).map(|record| record.sequence),
        Some(sequence + 1),
        "{name}: the redraw made a record"
    );
    sequence += 1;

    // `cd` changes the effective directory and the shell's own.
    let target = home.0.join("moved");
    fs::create_dir(&target).unwrap();
    shell.cycle(&format!("cd '{}'", target.display()));
    let record = newest(&shell).unwrap();
    assert_eq!(record.sequence, sequence + 1, "{name}: cd made a record");
    let canonical = target.canonicalize().unwrap();
    let same = |directory: Option<huterm_protocol::TerminalDirectory>| {
        directory.is_some_and(|directory| {
            directory.is_local()
                && Path::new(directory.path()).canonicalize().ok().as_ref()
                    == Some(&canonical)
        })
    };
    assert!(
        same(shell.directory.clone()),
        "{name}: {:?}",
        shell.directory
    );
    assert!(
        same(shell.reports.shell_directory()),
        "{name}: {:?}",
        shell.reports.shell_directory()
    );
    eprintln!("native {name}: command records ran");
}

#[test]
fn native_zsh_records() {
    run_records("zsh");
}

#[test]
fn native_bash_records() {
    run_records("bash");
}

#[cfg(target_os = "macos")]
#[test]
fn native_apple_bash_records() {
    run_records("/bin/bash");
}

#[test]
fn native_fish_records() {
    run_records("fish");
}

#[test]
fn native_nushell_records() {
    run_records("nu");
}

#[test]
fn native_elvish_records() {
    run_records("elvish");
}
