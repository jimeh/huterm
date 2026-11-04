# Huterm shell integration

Huterm wrote all of these scripts. No code from another project is vendored,
copied, or adapted, so no third-party notice applies. Kitty's, Ghostty's, and
iTerm2's shell integration scripts are GPL-licensed and were not consulted.

## References consulted

These permissively licensed projects were read for design and interoperability
only. Their licenses were checked first.

- [VS Code terminal shell integration](https://github.com/microsoft/vscode/tree/077bec303574782e7b6f09b94c761ca96c7068eb/src/vs/workbench/contrib/terminal/common/scripts)
  (MIT, commit `077bec303574782e7b6f09b94c761ca96c7068eb`):
  `shellIntegration-bash.sh`, `shellIntegration-rc.zsh`,
  `shellIntegration-env.zsh`, `shellIntegration-profile.zsh`, and
  `shellIntegration-login.zsh`. Huterm's zsh hooks follow its practice of
  restoring the undecorated prompt before each command. Huterm did not adopt
  its `ZDOTDIR` stub for every startup file: system files such as macOS's
  `/etc/zshrc` and Ubuntu's `/etc/zsh/zshrc` derive history and completion
  dump paths from `ZDOTDIR`.
- [bash-preexec](https://github.com/rcaloras/bash-preexec) (MIT), releases
  0.5.0 and 0.7.0 (commit `d866eeefdb8dfce075cbd4e37dc73c4deb4b0bd2`):
  read for its prompt-command layout (`__bp_precmd_invoke_cmd` first,
  `__bp_interactive_mode` last), its lazy installation, and how it wraps an
  existing `DEBUG` trap, so Huterm's entries keep that layout intact. The
  native test fixture emulates those boundaries in its own code.
- Shell sources, to confirm behavior the manuals leave open: zsh 5.9
  (`Src/exec.c`, `Src/jobs.c`, `Src/utils.c`; the zsh licence) for which
  hook contexts preserve `$?`, `pipestatus`, and `$_`, and bash 5.2 and 5.3
  (`parse.y`, `eval.c`; GPLv3, read only, nothing copied) for how `\#` is
  counted in `${var@P}`.

## Mechanisms

- [Bash startup files](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html),
  [POSIX mode](https://www.gnu.org/software/bash/manual/html_node/Bash-POSIX-Mode.html),
  prompt escapes and expansion, `PS0`, `PROMPT_COMMAND` (string and array
  forms), and the `DEBUG` trap. Bash restores `$?`, `$_`, and `PIPESTATUS`
  after `PROMPT_COMMAND`, so a completion marker in the prompt string reports
  the command's status. Huterm's marks write their semicolons as `\073` so it
  can remove exactly its own marks. Bash hooks never read history or change
  `HISTCONTROL`.
- [Zsh startup files](https://zsh.sourceforge.io/Doc/Release/Files.html),
  [hook functions](https://zsh.sourceforge.io/Doc/Release/Functions.html#Hook-Functions),
  prompt escapes, the [scheduler](https://zsh.sourceforge.io/Doc/Release/Zsh-Modules.html#The-zsh_002fsched-Module),
  and the `always` error boundary. Hook functions run with the command's
  `$?`, `pipestatus`, and `$_` preserved. Huterm's zsh marks end with ST
  rather than BEL so it can remove exactly its own marks.
- [Fish initialization](https://fishshell.com/docs/current/language.html#configuration-files),
  event handlers, `fish_preexec`, `fish_postexec`, and `fish_prompt`.
- [Nushell configuration](https://www.nushell.sh/book/configuration.html),
  vendor autoload, modules, `export-env`, `nu-check`, and
  [hooks](https://www.nushell.sh/book/hooks.html).
- [Elvish modules](https://elv.sh/ref/language.html#modules), `eval`, `try`,
  and the [editor's synchronous hooks](https://elv.sh/ref/edit.html).

`docs/configuration.md` describes the resulting behavior and its limitations.
Bootstraps restore the launch environment before loading optional bodies. Every
asset is data (mode 0644); only the user's selected shell executes it. Native
PTY tests pin marker behavior to the project-managed shell versions.
