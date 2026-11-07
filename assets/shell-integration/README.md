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
  `__bp_interactive_mode` last), its lazy installation, how it wraps an
  existing `DEBUG` trap, how it recognizes `PROMPT_COMMAND` lines and
  `;`-separated parts, and how 0.7.0 sanitizes `PROMPT_COMMAND` after it
  installs, so Huterm's entries keep that layout intact and keep 0.7.0's
  pending installer last. The native test fixtures emulate those boundaries in
  their own code.
- Prompt themes, run in a scratch home and read to find how each one sets and
  redraws `PROMPT` (all MIT): Powerlevel10k 1.20.0 (commit
  `35833ea15f14b71dbcebc7e54c104d8d56ca5268`), Pure 1.28.4 (commit
  `cac55d0b531a0db72294c9bba9453dfa6374c59d`), and Spaceship 4.22.6 (commit
  `0676854eb49585d6be2b4faf218de92bbf18ce18`). Starship 1.27.0's generated
  zsh init (ISC) was read too, and Oh My Zsh was only run.
- Shell sources, to confirm behavior the manuals leave open: zsh 5.9
  (`Src/exec.c`, `Src/jobs.c`, `Src/utils.c`; the zsh licence) for which
  hook contexts preserve `$?`, `pipestatus`, and `$_`; zsh 5.9's
  `Src/Zle/zle_main.c` and `Src/Zle/zle_refresh.c` for when `reset-prompt`
  expands the prompt and which refreshes run `zle-line-pre-redraw`; and bash
  5.2 and 5.3 (`parse.y`, `eval.c`; GPLv3, read only, nothing copied) for how
  `\#` is counted in `${var@P}`.

## Mechanisms

- [Bash startup files](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html)
  and `--rcfile`, prompt escapes and expansion, `PS0`, `PROMPT_COMMAND`
  (string and array forms), `case`, and the `DEBUG` trap. Bash restores `$?`,
  `$_`, and `PIPESTATUS` after `PROMPT_COMMAND`, so a completion marker in the
  prompt string reports the command's status. Bash never exports arrays, so
  prompt additions that expand Huterm's arrays are empty in a nested shell
  that inherits an exported `PS1` or `PS0`. Each `PROMPT_COMMAND` entry is a
  `case` on Huterm's per-process guard, which keeps `$_` and `$?` in a shell
  that lacks Huterm's functions. Huterm restores a prompt only while it holds
  the exact value Huterm set; its static `promptvars`-off marks write their
  semicolons as `\073` so that text is Huterm's alone. Bash hooks never read
  history or change `HISTCONTROL`.
- [Zsh startup files](https://zsh.sourceforge.io/Doc/Release/Files.html),
  [hook functions](https://zsh.sourceforge.io/Doc/Release/Functions.html#Hook-Functions),
  prompt escapes, the [scheduler](https://zsh.sourceforge.io/Doc/Release/Zsh-Modules.html#The-zsh_002fsched-Module),
  and the `always` error boundary. Hook functions run with the command's
  `$?`, `pipestatus`, and `$_` preserved. Huterm restores the undecorated
  prompt only while `PROMPT` holds the exact value Huterm set, so it never
  removes a framework's marks. A `zle` function wraps the builtin so that a
  theme's own `reset-prompt` expands a marked prompt.
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
