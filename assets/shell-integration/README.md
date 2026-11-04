# Huterm shell integration

Huterm's scripts were written for Huterm from the shells' documentation.
No other terminal's shell integration scripts were consulted or used.
All shipped scripts are Huterm-authored. Bash hooks use PS0 on Bash 4.4 and
later, and a DEBUG fallback on 3.2, without reading command history or
changing `HISTCONTROL`. This replaces bash-preexec, whose history-based
callbacks required weakening `ignorespace`.

Mechanisms used:

- [Bash startup files](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html),
  [POSIX mode](https://www.gnu.org/software/bash/manual/html_node/Bash-POSIX-Mode.html),
  prompt escapes, shell variables, arrays, PS0, and the DEBUG trap. A first
  `PROMPT_COMMAND` entry captures status, and a final entry decorates the prompt
  after user prompt construction, including on Bash 3.2.
- [Zsh startup files](https://zsh.sourceforge.io/Doc/Release/Files.html),
  [hook functions](https://zsh.sourceforge.io/Doc/Release/Functions.html#Hook-Functions),
  prompt escapes, ZLE line-init hooks, and the `always` error boundary. ZLE
  decorates the final prompt after all `precmd` hooks have run.
- [Fish initialization](https://fishshell.com/docs/current/language.html#configuration-files),
  event handlers, `fish_preexec`, `fish_postexec`, and `fish_prompt`.
- [Nushell configuration](https://www.nushell.sh/book/configuration.html),
  vendor autoload, modules, `export-env`, `nu-check`, and
  [hooks](https://www.nushell.sh/book/hooks.html).
- [Elvish modules](https://elv.sh/ref/language.html#modules), `eval`, `try`,
  and the [editor's synchronous hooks](https://elv.sh/ref/edit.html).

Bootstraps restore the launch environment before loading optional bodies. Every
asset is data (mode 0644); only the user's selected shell executes it. Native
PTY tests pin marker behavior to the project-managed shell versions.
