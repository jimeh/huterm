//! Executable-relative resources and a shell launch plan. No PTY parsing lives here.
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use huterm_protocol::{ShellFeature, ShellIntegrationMode, TerminalCommand};

const OWNED: &[&str] = &[
    "HUTERM_RESOURCES_DIR",
    "HUTERM_BIN_DIR",
    "HUTERM_SHELL_FEATURES",
    "HUTERM_SHELL_INTEGRATION",
    "HUTERM_BASH_INJECT",
    "HUTERM_BASH_LOGIN",
    "HUTERM_BASH_RCFILE",
    "HUTERM_ORIG_ENV",
    "HUTERM_ORIG_HISTFILE",
    "HUTERM_ORIG_ZDOTDIR",
    "HUTERM_ORIG_XDG_DATA_DIRS",
    "HUTERM_XDG_ENTRY",
];

type Resolver<'a> =
    dyn Fn(&Path, &Path, Option<&OsStr>) -> Option<PathBuf> + 'a;

pub(crate) struct Host<'a> {
    pub resources: Option<PathBuf>,
    pub bin: Option<PathBuf>,
    pub macos: bool,
    /// Effective child environment, including client overrides.
    pub environment: &'a dyn Fn(&str) -> Option<OsString>,
    pub resolve: &'a Resolver<'a>,
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
        "bash" => {
            let apple = host.macos
                && (host.resolve)(
                    &command.program,
                    &command.working_directory,
                    (host.environment)("PATH").as_deref(),
                )
                .is_some_and(|file| file == Path::new("/bin/bash"));
            bash_plan(command, host, &mut result, &bootstrap, apple);
        }
        "fish" | "nu" | "elvish" => {
            let root =
                bootstrap.ancestors().nth(if shell == "nu" { 4 } else { 3 });
            if let Some(root) = root {
                result.save(host, "XDG_DATA_DIRS");
                result.set("HUTERM_XDG_ENTRY", root);
                let mut value = root.as_os_str().to_owned();
                value.push(":");
                value.push((host.environment)("XDG_DATA_DIRS").unwrap_or_else(
                    || OsString::from("/usr/local/share:/usr/share"),
                ));
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
    arguments.iter().any(|arg| {
        arg == "-c"
            || arg.starts_with("--command")
            || arg == "--lsp"
            || (shell == "bash" && arg == "--posix")
            || (arg.starts_with('-')
                && !arg.starts_with("--")
                && arg[1..].contains('c'))
    })
}

fn bash_plan(
    command: &TerminalCommand,
    host: &Host<'_>,
    result: &mut LaunchPlan,
    bootstrap: &Path,
    apple: bool,
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
            "--login" if apple => result.set("HUTERM_BASH_LOGIN", "1"),
            value
                if apple
                    && value.starts_with('-')
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
    result.save(host, "ENV");
    result.save(host, "HISTFILE");
    if apple {
        result.arguments = vec![
            "--rcfile".into(),
            bootstrap.to_string_lossy().into_owned(),
            "-i".into(),
        ];
    } else {
        result.set("ENV", bootstrap);
        // POSIX bash otherwise defaults history to ~/.sh_history before ENV.
        result.set(
            "HISTFILE",
            (host.environment)("HISTFILE").unwrap_or_default(),
        );
        result.arguments = vec!["--posix".into()];
    }
    result.arguments.extend(arguments);
}

/// Match portable-pty's effective-PATH and explicit ./ or ../ resolution,
/// then canonicalize only for the Apple bash identity check.
pub(crate) fn resolve(
    program: &Path,
    cwd: &Path,
    search: Option<&OsStr>,
) -> Option<PathBuf> {
    let executable = |file: &Path| {
        #[cfg(unix)]
        {
            nix::unistd::access(file, nix::unistd::AccessFlags::X_OK).is_ok()
                && file.is_file()
        }
        #[cfg(not(unix))]
        {
            file.is_file()
        }
    };
    let resolved = if program.is_absolute() {
        program.to_owned()
    } else if matches!(
        program.components().next(),
        Some(Component::CurDir | Component::ParentDir)
    ) {
        cwd.join(program)
    } else {
        std::env::split_paths(search?)
            .map(|part| cwd.join(part).join(program))
            .find(|file| executable(file))?
    };
    executable(&resolved)
        .then(|| resolved.canonicalize().ok())
        .flatten()
}

pub(crate) fn resources(executable: &Path, macos: bool) -> Option<PathBuf> {
    resource_candidates(executable, macos)
        .into_iter()
        .find(|directory| directory.is_dir())
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
    let profile = if matches!(
        bin.file_name().and_then(OsStr::to_str),
        Some("examples" | "deps")
    ) {
        bin.parent()
    } else {
        Some(bin)
    };
    if let Some(root) = profile.and_then(Path::parent).and_then(Path::parent) {
        candidates.push(root.join("assets"));
    }
    candidates
}

#[cfg(all(test, unix))]
mod native_tests;
#[cfg(test)]
mod tests;
