//! Executable-relative resources and a shell launch plan. No PTY parsing lives here.
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use huterm_protocol::{ShellFeature, ShellIntegrationMode, TerminalCommand};

const OWNED: &[&str] = &[
    "HUTERM_RESOURCES_DIR",
    "HUTERM_BIN_DIR",
    "HUTERM_SHELL_FEATURES",
    "HUTERM_SHELL_INTEGRATION",
    "HUTERM_BASH_INJECT",
    "HUTERM_BASH_LOGIN",
    "HUTERM_BASH_RCFILE",
    "HUTERM_ORIG_ZDOTDIR",
    "HUTERM_ORIG_XDG_DATA_DIRS",
    "HUTERM_XDG_ENTRY",
];

pub(crate) struct Host<'a> {
    pub resources: Option<PathBuf>,
    pub bin: Option<PathBuf>,
    /// Effective child environment, including client overrides.
    pub environment: &'a dyn Fn(&str) -> Option<OsString>,
    pub is_file: &'a dyn Fn(&Path) -> bool,
}

#[derive(Debug)]
pub(crate) struct LaunchPlan {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub set: Vec<(String, OsString)>,
    pub remove: Vec<&'static str>,
    pub warning: Option<String>,
}

impl LaunchPlan {
    fn set(&mut self, name: &str, value: impl Into<OsString>) {
        self.set.push((name.into(), value.into()));
    }

    // Prefixing the value preserves unset versus empty without shell quoting.
    fn save(&mut self, host: &Host<'_>, name: &str) {
        let mut encoded = OsString::from("0");
        if let Some(value) = (host.environment)(name) {
            encoded = OsString::from("1");
            encoded.push(value);
        }
        self.set(&format!("HUTERM_ORIG_{name}"), encoded);
    }
}

pub(crate) fn plan(command: &TerminalCommand, host: &Host<'_>) -> LaunchPlan {
    let mut result = LaunchPlan {
        program: command.program.clone(),
        arguments: command.arguments.clone(),
        set: Vec::new(),
        remove: OWNED.to_vec(),
        warning: None,
    };
    if let Some(resources) = &host.resources {
        result.set("HUTERM_RESOURCES_DIR", resources);
    }
    if let Some(bin) = &host.bin {
        result.set("HUTERM_BIN_DIR", bin);
    }
    if command.shell_integration.mode == ShellIntegrationMode::Off {
        return result;
    }
    let Some(shell) = command.program.file_name().and_then(OsStr::to_str)
    else {
        return result;
    };
    if !matches!(shell, "bash" | "zsh" | "fish" | "nu" | "elvish")
        || noninteractive(shell, &command.arguments)
    {
        return result;
    }
    let bootstrap = match shell {
        "bash" => "bash/bootstrap.bash",
        "zsh" => "zsh/.zshenv",
        "fish" => "fish/xdg/fish/vendor_conf.d/huterm.fish",
        "nu" => "nushell/xdg/nushell/vendor/autoload/huterm.nu",
        "elvish" => "elvish/xdg/elvish/lib/huterm-integration.elv",
        _ => unreachable!(),
    };
    let bootstrap = host
        .resources
        .as_ref()
        .map(|root| root.join("shell-integration").join(bootstrap));
    let Some(bootstrap) = bootstrap.filter(|file| (host.is_file)(file)) else {
        result.warning = Some(format!(
            "huterm: shell integration bootstrap missing for {shell}; starting plain"
        ));
        return result;
    };
    result.set("HUTERM_SHELL_INTEGRATION", shell);
    if command
        .shell_integration
        .features
        .contains(ShellFeature::Path)
    {
        result.set("HUTERM_SHELL_FEATURES", "path");
    }
    match shell {
        "zsh" => {
            result.save(host, "ZDOTDIR");
            result.set("ZDOTDIR", bootstrap.parent().unwrap_or(Path::new("")));
        }
        "bash" => bash_plan(command, &mut result, &bootstrap),
        "fish" | "nu" | "elvish" => {
            let root =
                bootstrap.ancestors().nth(if shell == "nu" { 4 } else { 3 });
            if let Some(root) = root {
                result.save(host, "XDG_DATA_DIRS");
                result.set("HUTERM_XDG_ENTRY", root);
                let mut value = root.as_os_str().to_owned();
                value.push(":");
                value.push(
                    (host.environment)("XDG_DATA_DIRS")
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| {
                            OsString::from("/usr/local/share:/usr/share")
                        }),
                );
                result.set("XDG_DATA_DIRS", value);
            }
            if shell == "nu" {
                result
                    .arguments
                    .extend(["--execute".into(), "use huterm *".into()]);
            }
        }
        _ => {}
    }
    result
}

fn noninteractive(shell: &str, arguments: &[String]) -> bool {
    // Parse invocation options, not arbitrary letters in single-dash long names.
    // Options taking a value must consume it before looking for a script operand.
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().is_some();
        }
        let option_prefix = arg.starts_with('-')
            || (matches!(shell, "bash" | "zsh") && arg.starts_with('+'));
        if !option_prefix || arg == "-" {
            return true;
        }
        let (option, attached) = arg
            .split_once('=')
            .map_or((arg.as_str(), false), |(key, _)| (key, true));
        if exits_before_prompt(shell, option) {
            return true;
        }
        let takes_value = option_takes_value(shell, option);
        if takes_value {
            if !attached {
                args.next();
            }
            continue;
        }
        if matches!(shell, "bash" | "zsh" | "fish") {
            if matches!(option, "--command" | "--commands") {
                return true;
            }
            if arg.starts_with('-') && !arg.starts_with("--") {
                // o/O (bash/zsh) and C (fish) consume the remaining letters or
                // the next argument as their value, rather than more flags.
                for (index, flag) in arg[1..].char_indices() {
                    if flag == 'c' {
                        return true;
                    }
                    if (matches!(shell, "bash" | "zsh")
                        && matches!(flag, 'o' | 'O'))
                        || (shell == "fish"
                            && matches!(flag, 'C' | 'd' | 'o' | 'p'))
                    {
                        if index + 2 == arg.len() {
                            args.next();
                        }
                        break;
                    }
                }
            }
        }
    }
    false
}

fn exits_before_prompt(shell: &str, option: &str) -> bool {
    match shell {
        "bash" => matches!(
            option,
            "--posix"
                | "--help"
                | "--version"
                | "--dump-strings"
                | "--dump-po-strings"
        ),
        "nu" => {
            matches!(
                option,
                "-c" | "--commands"
                    | "--lsp"
                    | "--dap"
                    | "--mcp"
                    | "--help"
                    | "-h"
                    | "--version"
                    | "-v"
            ) || option.starts_with("--ide-")
        }
        "elvish" => matches!(
            option,
            "-c" | "-compileonly"
                | "-lsp"
                | "-daemon"
                | "-help"
                | "-version"
                | "-buildinfo"
        ),
        _ => false,
    }
}

fn option_takes_value(shell: &str, option: &str) -> bool {
    match shell {
        "bash" => matches!(
            option,
            "--rcfile" | "--init-file" | "-o" | "+o" | "-O" | "+O"
        ),
        "zsh" => matches!(option, "-o" | "+o"),
        "fish" => matches!(
            option,
            "--init-command"
                | "--features"
                | "--debug"
                | "--debug-output"
                | "--profile"
                | "--profile-startup"
                | "-C"
                | "-d"
                | "-o"
                | "-p"
        ),
        "nu" => matches!(
            option,
            "-e" | "--execute"
                | "-I"
                | "--include-path"
                | "-m"
                | "--table-mode"
                | "--error-style"
                | "--config"
                | "--env-config"
                | "--config-home"
                | "--log-level"
                | "--log-target"
                | "--log-file"
                | "--threads"
                | "-t"
        ),
        "elvish" => matches!(
            option,
            "-rc" | "-db" | "-log" | "-sock" | "-deprecation-level"
        ),
        _ => false,
    }
}

// Every injected bash, Apple's 3.2 included, reads the bootstrap as its rcfile.
// A login start becomes an interactive one whose bootstrap emulates login.
fn bash_plan(
    command: &TerminalCommand,
    result: &mut LaunchPlan,
    bootstrap: &Path,
) {
    let mut arguments = Vec::new();
    let mut flags = String::new();
    let mut iter = command.arguments.iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--norc" => flags.push('r'),
            "--noprofile" => flags.push('p'),
            "--rcfile" | "--init-file" => {
                if let Some(file) = iter.next() {
                    result.set("HUTERM_BASH_RCFILE", file);
                }
            }
            "--login" => result.set("HUTERM_BASH_LOGIN", "1"),
            value
                if value.starts_with('-')
                    && !value.starts_with("--")
                    && value.contains('l') =>
            {
                result.set("HUTERM_BASH_LOGIN", "1");
                let value = value.replace('l', "");
                if value != "-" {
                    arguments.push(value);
                }
            }
            _ => arguments.push(argument.clone()),
        }
    }
    result.set("HUTERM_BASH_INJECT", flags);
    // Bash accepts long options only before single-character ones.
    let long = arguments
        .iter()
        .take_while(|argument| argument.starts_with("--") && *argument != "--")
        .count();
    arguments.insert(long, "-i".into());
    result.arguments =
        vec!["--rcfile".into(), bootstrap.to_string_lossy().into_owned()];
    result.arguments.extend(arguments);
}

pub(crate) fn resources(executable: &Path, macos: bool) -> Option<PathBuf> {
    resource_candidates(executable, macos)
        .into_iter()
        .find(|directory| {
            // Packaged identity remains useful even if its bootstrap is missing.
            // Development candidates must contain the payload, not just assets.
            if directory.file_name() == Some(OsStr::new("assets")) {
                directory.join("shell-integration").is_dir()
            } else {
                directory.is_dir()
            }
        })
}

fn resource_candidates(executable: &Path, macos: bool) -> Vec<PathBuf> {
    let Some(bin) = executable.parent() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    if let Some(parent) = bin.parent() {
        if macos {
            candidates.push(parent.join("Resources"));
        }
        candidates.push(parent.join("share/huterm"));
    }
    // Custom target directories can add several levels below the repository.
    // Keep installed resource layouts ahead of development discovery.
    for ancestor in bin.ancestors().take(6) {
        candidates.push(ancestor.join("assets"));
    }
    candidates
}

#[cfg(all(test, unix))]
mod native_tests;
#[cfg(test)]
mod tests;
