const NAME_BYTE_LIMIT: usize = 256;

/// Programs that commonly run a script named by their first argument.
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "csh", "tcsh", "node",
    "nodejs", "deno", "bun", "ruby", "perl", "php", "lua", "luajit", "tclsh",
    "pwsh",
];

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "csh", "tcsh", "nu",
    "xonsh", "elvish", "pwsh",
];

/// Names a process the way its user most likely invoked it.
///
/// Uses the basename of argv[0] without a login shell's leading `-`. When that
/// names an interpreter and argv[1] is a path rather than an option, names the
/// script instead: kernels report the interpreter for shebang scripts, and
/// `#!/usr/bin/env` scripts re-exec it. Returns `None` for an empty name.
#[must_use]
pub fn display_name(arguments: &[String]) -> Option<String> {
    let program = basename(arguments.first()?).trim_start_matches('-');
    let script = arguments
        .get(1)
        .filter(|argument| {
            is_interpreter(program)
                && !argument.is_empty()
                && !argument.starts_with('-')
        })
        .map(|argument| basename(argument));
    let name = script.unwrap_or(program);
    (!name.is_empty()).then(|| truncate(name).to_owned())
}

/// Whether a display name is an interactive shell.
#[must_use]
pub fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name)
}

fn is_interpreter(name: &str) -> bool {
    INTERPRETERS.contains(&name) || name.starts_with("python")
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

fn truncate(name: &str) -> &str {
    let mut end = name.len().min(NAME_BYTE_LIMIT);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(arguments: &[&str]) -> Option<String> {
        display_name(
            &arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn programs_use_their_invoked_basename() {
        assert_eq!(
            name(&["/usr/bin/vim", "notes.txt"]).as_deref(),
            Some("vim")
        );
        assert_eq!(name(&["vi"]).as_deref(), Some("vi"));
        assert_eq!(name(&["-zsh"]).as_deref(), Some("zsh"));
        assert_eq!(name(&["/bin/zsh", "-l"]).as_deref(), Some("zsh"));
    }

    #[test]
    fn interpreters_name_their_script() {
        assert_eq!(
            name(&["node", "/usr/local/bin/npm", "install"]).as_deref(),
            Some("npm")
        );
        assert_eq!(
            name(&["/bin/sh", "./deploy.sh"]).as_deref(),
            Some("deploy.sh")
        );
        assert_eq!(
            name(&["python3.14", "/opt/tools/bin/httpie"]).as_deref(),
            Some("httpie")
        );
    }

    #[test]
    fn interpreter_options_and_missing_scripts_keep_the_interpreter() {
        assert_eq!(
            name(&["python3", "-m", "http.server"]).as_deref(),
            Some("python3")
        );
        assert_eq!(name(&["bash", "-c", "make"]).as_deref(), Some("bash"));
        assert_eq!(name(&["node"]).as_deref(), Some("node"));
        assert_eq!(name(&["ruby", ""]).as_deref(), Some("ruby"));
    }

    #[test]
    fn empty_names_and_long_names_are_bounded() {
        assert_eq!(name(&[]), None);
        assert_eq!(name(&[""]), None);
        assert_eq!(name(&["-"]), None);
        let long = "é".repeat(200);
        let truncated = name(&[&long]).unwrap();
        assert_eq!(truncated.len(), NAME_BYTE_LIMIT);
        assert!(truncated.chars().all(|character| character == 'é'));
    }

    #[test]
    fn shells_are_recognised_by_display_name() {
        for shell in ["sh", "zsh", "mksh", "nu", "xonsh", "elvish", "pwsh"] {
            assert!(is_shell(shell), "{shell}");
        }
        assert!(!is_shell("deploy.sh"));
        assert!(!is_shell("vim"));
    }
}
