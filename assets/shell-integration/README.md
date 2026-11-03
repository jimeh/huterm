# Huterm shell integration

Huterm's scripts were written for Huterm from the shells' documentation.
No other terminal's shell integration scripts were consulted or used.
`bash/bash-preexec.sh` is the sole vendored script, unmodified; its version,
source, SHA-256 checksums, and MIT notice are in
[`third-party/bash-preexec`](../../third-party/bash-preexec/README.md).

Mechanisms used:

- [Bash startup files](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html),
  [POSIX mode](https://www.gnu.org/software/bash/manual/html_node/Bash-POSIX-Mode.html),
  prompt escapes, shell variables, and arrays. bash-preexec supplies cooperative
  preexec and precmd hooks, including Bash 3.2 support.
- [Zsh startup files](https://zsh.sourceforge.io/Doc/Release/Files.html),
  [hook functions](https://zsh.sourceforge.io/Doc/Release/Functions.html#Hook-Functions),
  prompt escapes, and the `always` error boundary.
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
