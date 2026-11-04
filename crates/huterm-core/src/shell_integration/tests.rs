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
            environment: &lookup,
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
            ("fish", "XDG_DATA_DIRS"),
            ("nu", "XDG_DATA_DIRS"),
            ("elvish", "XDG_DATA_DIRS"),
        ] {
            let env = original
                .map(|value| vec![(name, value)])
                .unwrap_or_default();
            let mut command = command(shell);
            let plan = planned(&command, &env, true);
            assert_eq!(
                value(&plan, &format!("HUTERM_ORIG_{name}")),
                Some(OsStr::new(
                    &original.map_or("0".into(), |value| format!("1{value}"))
                ))
            );
            assert_eq!(value(&plan, "HUTERM_SHELL_FEATURES"), None);
            command.environment.push((name.into(), "override".into()));
            let plan = planned(&command, &env, true);
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
        for environment in [vec![], vec![("XDG_DATA_DIRS", "")]] {
            let plan = planned(&command(shell), &environment, true);
            let root = format!("/resources/shell-integration/{root}/xdg");
            assert_eq!(
                value(&plan, "HUTERM_XDG_ENTRY"),
                Some(OsStr::new(&root))
            );
            assert_eq!(
                value(&plan, "XDG_DATA_DIRS"),
                Some(OsStr::new(&format!(
                    "{root}:/usr/local/share:/usr/share"
                )))
            );
        }
    }
}

#[test]
fn planner_bash_uses_rcfile_bootstrap_and_emulates_login() {
    let bootstrap = "/resources/shell-integration/bash/bootstrap.bash";
    for (arguments, expected, login) in [
        (vec![], vec!["-i"], false),
        (vec!["-l"], vec!["-i"], true),
        (vec!["--login"], vec!["-i"], true),
        (
            vec!["-il", "-O", "extglob"],
            vec!["-i", "-i", "-O", "extglob"],
            true,
        ),
        (
            vec!["--noediting", "--login", "-o", "vi"],
            vec!["--noediting", "-i", "-o", "vi"],
            true,
        ),
        (
            vec!["-il", "--noprofile", "--init-file", "/custom rc"],
            vec!["-i", "-i"],
            true,
        ),
    ] {
        let mut command = command("bash");
        command.arguments =
            arguments.iter().map(|&value| value.into()).collect();
        let plan = planned(
            &command,
            &[("ENV", "/user env"), ("HISTFILE", "/user history")],
            true,
        );
        assert_eq!(plan.program, command.program);
        let mut full = vec!["--rcfile", bootstrap];
        full.extend(&expected);
        assert_eq!(plan.arguments, full, "{arguments:?}");
        assert_eq!(
            value(&plan, "HUTERM_BASH_LOGIN"),
            login.then_some(OsStr::new("1")),
            "{arguments:?}"
        );
        // Bash's own ENV and HISTFILE handling applies untouched.
        assert!(
            plan.set.iter().all(|(name, _)| !matches!(
                name.as_str(),
                "ENV" | "HISTFILE"
            ) && !name
                .starts_with("HUTERM_ORIG_")),
            "{:?}",
            plan.set
        );
    }
    let mut command = command("bash");
    command.arguments = ["--noprofile", "--rcfile", "/custom rc"]
        .map(str::to_owned)
        .into();
    let plan = planned(&command, &[], true);
    assert_eq!(value(&plan, "HUTERM_BASH_INJECT"), Some(OsStr::new("p")));
    assert_eq!(
        value(&plan, "HUTERM_BASH_RCFILE"),
        Some(OsStr::new("/custom rc"))
    );
    let plan = planned(&super::tests::command("sh"), &[], true);
    assert_eq!(plan.arguments, [] as [String; 0]);
}

#[test]
fn planner_missing_bootstrap_starts_plain() {
    let command = command("zsh");
    let plan = planned(&command, &[], false);
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
        value(&planned(&command, &[], true), "HUTERM_SHELL_FEATURES"),
        Some(OsStr::new("path"))
    );
    for binary in [
        "/repo/target/debug/huterm",
        "/repo/target/release/huterm",
        "/repo/target/debug/examples/smoke",
        "/repo/target/debug/deps/test",
        "/repo/target/ghostty-releasesafe/debug/deps/test",
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

#[test]
fn planner_distinguishes_options_from_command_and_script_invocations() {
    for (shell, args, skip) in [
        ("bash", vec!["script.sh"], true),
        ("bash", vec!["--rcfile", "custom", "-i"], false),
        ("bash", vec!["--rcfile", "", "-i"], false),
        ("bash", vec!["-O", "extglob", "-i"], false),
        // An rcfile start cannot honor --norc on Debian-family bash.
        ("bash", vec!["--norc"], true),
        ("bash", vec!["--norc", "-i"], true),
        ("bash", vec!["-ic", "true"], true),
        ("zsh", vec!["-ilc", "true"], true),
        ("zsh", vec!["--", "script.zsh"], true),
        ("zsh", vec!["-o", "emacs", "-i"], false),
        ("fish", vec!["-ic", "true"], true),
        ("fish", vec!["--init-command", "true", "-i"], false),
        ("fish", vec!["script.fish"], true),
        ("fish", vec!["+script.fish"], true),
        ("nu", vec!["script.nu"], true),
        ("nu", vec!["+script.nu"], true),
        ("nu", vec!["--commands=true"], true),
        ("nu", vec!["-c", "true"], true),
        ("nu", vec!["--config", "config.nu"], false),
        ("elvish", vec!["-norc"], false),
        ("elvish", vec!["-rc", "custom.elv"], false),
        ("elvish", vec!["-c", "true"], true),
        ("elvish", vec!["-compileonly", "script.elv"], true),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(noninteractive(shell, &args), skip, "{shell} {args:?}");
    }
}

#[test]
fn resources_find_assets_above_nested_target_directories() {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir()
        .join(format!("huterm-shell-resources-{}", std::process::id()));
    let _cleanup = Cleanup(root.clone());
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let binary = root.join("target/ghostty-releasesafe/debug/deps/core-test");
    assert_eq!(
        resources(&binary, false),
        None,
        "an assets directory alone is not a shell payload"
    );
    std::fs::create_dir(assets.join("shell-integration")).unwrap();
    assert_eq!(resources(&binary, false), Some(assets));
    let packaged = root.join("app/share/huterm");
    std::fs::create_dir_all(&packaged).unwrap();
    assert_eq!(
        resources(&root.join("app/bin/huterm"), false),
        Some(packaged),
        "packaged identity wins even when its bootstrap is missing"
    );
}
