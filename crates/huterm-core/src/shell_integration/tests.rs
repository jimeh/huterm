use super::*;
use huterm_protocol::{
    CellSize, GridSize, ScrollbackLimit, ShellIntegration, TerminalPresentation,
};
use std::collections::BTreeMap;

pub(super) fn command(shell: &str) -> TerminalCommand {
    TerminalCommand {
        program: shell.into(),
        arguments: Vec::new(),
        working_directory: "/work".into(),
        environment: Vec::new(),
        shell_integration: ShellIntegration::default(),
        grid_size: GridSize::clamped(100, 30),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: TerminalPresentation::default(),
        scrollback: ScrollbackLimit::DEFAULT,
        scrollback_compression: false,
    }
}

fn planned(
    command: &TerminalCommand,
    env: &[(&str, &str)],
    macos: bool,
    resolved: &str,
    exists: bool,
) -> LaunchPlan {
    let env: BTreeMap<_, _> = env
        .iter()
        .map(|(name, value)| (*name, OsString::from(value)))
        .collect();
    let lookup = |name: &str| {
        command
            .environment
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into())
            .or_else(|| env.get(name).cloned())
    };
    plan(
        command,
        &Host {
            resources: Some("/resources".into()),
            bin: Some("/bin/huterm".into()),
            macos,
            environment: &lookup,
            resolve: &|_, _, _| Some(resolved.into()),
            is_file: &|_| exists,
        },
    )
}

fn value<'a>(plan: &'a LaunchPlan, name: &str) -> Option<&'a OsStr> {
    plan.set
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_os_str())
}

#[test]
fn planner_off_unsupported_and_noninteractive_preserve_original_launch() {
    for (shell, args, off) in [
        ("sh", vec![], false),
        ("zsh", vec!["-ic", "true"], false),
        ("bash", vec!["--posix"], false),
        ("nu", vec!["--commands", "true"], false),
        ("nu", vec!["--lsp"], false),
        ("fish", vec![], true),
    ] {
        let mut command = command(shell);
        command.arguments = args.into_iter().map(str::to_owned).collect();
        if off {
            command.shell_integration = ShellIntegration::OFF;
        }
        let plan = planned(
            &command,
            &[
                ("HUTERM_SHELL_INTEGRATION", "zsh"),
                ("HUTERM_SHELL_FEATURES", "path"),
            ],
            false,
            "/bin/bash",
            true,
        );
        assert_eq!(plan.program, command.program);
        assert_eq!(plan.arguments, command.arguments);
        assert_eq!(plan.set.len(), 2);
        assert_eq!(plan.remove, OWNED);
        assert!(plan.warning.is_none());
    }
}

#[test]
fn planner_preserves_unset_empty_and_client_overrides() {
    for original in [None, Some(""), Some("/user value")] {
        for (shell, name) in [
            ("zsh", "ZDOTDIR"),
            ("bash", "ENV"),
            ("bash", "HISTFILE"),
            ("fish", "XDG_DATA_DIRS"),
            ("nu", "XDG_DATA_DIRS"),
            ("elvish", "XDG_DATA_DIRS"),
        ] {
            let env = original
                .map(|value| vec![(name, value)])
                .unwrap_or_default();
            let mut command = command(shell);
            let plan = planned(&command, &env, false, "/opt/bash", true);
            assert_eq!(
                value(&plan, &format!("HUTERM_ORIG_{name}")),
                Some(OsStr::new(
                    &original.map_or("0".into(), |value| format!("1{value}"))
                ))
            );
            assert_eq!(value(&plan, "HUTERM_SHELL_FEATURES"), None);
            command.environment.push((name.into(), "override".into()));
            let plan = planned(&command, &env, false, "/opt/bash", true);
            assert_eq!(
                value(&plan, &format!("HUTERM_ORIG_{name}")),
                Some(OsStr::new("1override"))
            );
        }
    }
}

#[test]
fn planner_uses_shell_specific_xdg_roots_and_default_search_path() {
    for (shell, root) in
        [("fish", "fish"), ("nu", "nushell"), ("elvish", "elvish")]
    {
        let plan = planned(&command(shell), &[], false, "", true);
        let root = format!("/resources/shell-integration/{root}/xdg");
        assert_eq!(value(&plan, "HUTERM_XDG_ENTRY"), Some(OsStr::new(&root)));
        assert_eq!(
            value(&plan, "XDG_DATA_DIRS"),
            Some(OsStr::new(&format!("{root}:/usr/local/share:/usr/share")))
        );
    }
}

#[test]
fn planner_apple_bash_carries_startup_options_and_combined_login_flags() {
    for program in ["bash", "./bash", "/link/bash"] {
        let mut command = command(program);
        command.arguments =
            ["-il", "--norc", "--noprofile", "--init-file", "/custom rc"]
                .map(str::to_owned)
                .into();
        let plan = planned(&command, &[], true, "/bin/bash", true);
        assert_eq!(plan.program, command.program);
        assert_eq!(
            plan.arguments,
            [
                "--rcfile",
                "/resources/shell-integration/bash/bootstrap.bash",
                "-i",
                "-i"
            ]
        );
        assert_eq!(value(&plan, "HUTERM_BASH_LOGIN"), Some(OsStr::new("1")));
        assert_eq!(value(&plan, "HUTERM_BASH_INJECT"), Some(OsStr::new("rp")));
        assert_eq!(
            value(&plan, "HUTERM_BASH_RCFILE"),
            Some(OsStr::new("/custom rc"))
        );
    }
    let plan = planned(&command("sh"), &[], true, "/bin/bash", true);
    assert_eq!(plan.arguments, [] as [String; 0]);
    let plan = planned(&command("bash"), &[], true, "/opt/bash", true);
    assert_eq!(plan.arguments, ["--posix"]);
}

#[test]
fn planner_missing_bootstrap_warns_once_without_injection() {
    let command = command("zsh");
    let plan = planned(&command, &[], false, "", false);
    assert!(plan.warning.unwrap().contains("starting plain"));
    assert_eq!(plan.arguments, command.arguments);
    assert_eq!(plan.set.len(), 2);
}

#[test]
fn planner_path_is_opt_in_and_resources_are_executable_relative() {
    let mut command = command("bash");
    command.shell_integration.features =
        [ShellFeature::Path].into_iter().collect();
    assert_eq!(
        value(
            &planned(&command, &[], false, "", true),
            "HUTERM_SHELL_FEATURES"
        ),
        Some(OsStr::new("path"))
    );
    for binary in [
        "/repo/target/debug/huterm",
        "/repo/target/release/huterm",
        "/repo/target/debug/examples/smoke",
        "/repo/target/debug/deps/test",
    ] {
        assert!(
            resource_candidates(Path::new(binary), false)
                .contains(&PathBuf::from("/repo/assets"))
        );
    }
    assert!(
        resource_candidates(Path::new("/app/Contents/MacOS/huterm"), true)
            .contains(&PathBuf::from("/app/Contents/Resources"))
    );
    assert!(
        resource_candidates(Path::new("/app/bin/huterm"), false)
            .contains(&PathBuf::from("/app/share/huterm"))
    );
}

#[cfg(unix)]
#[test]
fn planner_resolver_matches_child_path_cwd_and_invocation_identity() {
    use std::os::unix::fs::symlink;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir()
        .join(format!("huterm-shell-resolver-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let _cleanup = Cleanup(root.clone());
    symlink("/bin/bash", root.join("bash")).unwrap();
    symlink("/bin/bash", root.join("sh")).unwrap();
    for program in ["bash", "./bash"] {
        assert_eq!(
            resolve(Path::new(program), &root, Some(OsStr::new("."))),
            Some(PathBuf::from("/bin/bash").canonicalize().unwrap())
        );
    }
    let mut command = command("bash");
    command.working_directory.clone_from(&root);
    command
        .environment
        .push(("PATH".into(), root.to_string_lossy().into_owned()));
    let environment = |name: &str| {
        command
            .environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into())
    };
    let host = Host {
        resources: Some("/resources".into()),
        bin: None,
        macos: true,
        environment: &environment,
        resolve: &resolve,
        is_file: &|_| true,
    };
    let planned = plan(&command, &host);
    #[cfg(target_os = "macos")]
    assert_eq!(planned.arguments[0], "--rcfile");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(planned.arguments[0], "--posix");
    command.program = root.join("sh");
    assert_eq!(
        plan(
            &command,
            &Host {
                resources: None,
                bin: None,
                macos: true,
                environment: &|_| None,
                resolve: &resolve,
                is_file: &|_| true
            }
        )
        .arguments,
        [] as [String; 0]
    );
}
