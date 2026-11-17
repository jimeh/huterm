# Huterm Nushell bootstrap. Vendor autoload runs after the user's startup files.
module huterm {
    # Features: ssh_env, ssh_terminfo, sudo. Huterm's `--execute` imports
    # `ssh` when ssh_env or ssh_terminfo is on and `sudo` when sudo is on,
    # after the user's startup files, so they replace a user's own `ssh` or
    # `sudo`. Each call reads the features again and runs the program itself
    # when they are off or the huterm command is missing.
    def huterm-command [] {
        let bin = ($env.HUTERM_BIN_DIR? | default '')
        if $bin == '' { return null }
        let file = ($bin | path join 'huterm')
        if (which $file | is-empty) { null } else { $file }
    }
    def features [] {
        $env.HUTERM_SHELL_FEATURES? | default '' | split row ','
    }
    # `huterm ssh` with the enabled features' flags, or ssh itself.
    def ssh-command [] {
        let flags = [[feature flag]; [ssh_env --forward-env] [ssh_terminfo --terminfo]]
            | where feature in (features) | get flag
        let huterm = (huterm-command)
        if ($huterm == null) or ($flags | is-empty) { [ssh] } else { [$huterm ssh ...$flags] }
    }
    # `huterm sudo` when the feature is on, or sudo itself.
    def sudo-command [] {
        let huterm = (huterm-command)
        if ($huterm == null) or ('sudo' not-in (features)) { [sudo] } else { [$huterm sudo] }
    }
    # Only a body's first pipeline receives the command's input, so the
    # external runs there: piped data reaches it, and without a pipe it
    # reads the terminal.
    export def --wrapped ssh [...arguments] {
        run-external ...(ssh-command) ...$arguments
    }
    export def --wrapped sudo [...arguments] {
        run-external ...(sudo-command) ...$arguments
    }
    export-env {
        if ($env.HUTERM_SHELL_INTEGRATION? | default '') == 'nu' {
            hide-env HUTERM_SHELL_INTEGRATION
            let original = ($env.HUTERM_ORIG_XDG_DATA_DIRS? | default '0')
            let saved = ($original | str substring 1..)
            let base = if $saved == '' { '/usr/local/share:/usr/share' } else { $saved }
            let entry = $env.HUTERM_XDG_ENTRY
            let current = ($env.XDG_DATA_DIRS? | default '')
            if $current == ($entry + ':' + $base) {
                if ($original | str starts-with '1') {
                    $env.XDG_DATA_DIRS = $saved
                } else {
                    hide-env -i XDG_DATA_DIRS
                }
            } else if $env.XDG_DATA_DIRS? != null {
                $env.XDG_DATA_DIRS = ($current | split row ':' | where {|part| $part != $entry } | str join ':')
            }
            hide-env -i HUTERM_ORIG_XDG_DATA_DIRS HUTERM_XDG_ENTRY
            let body = ($env.HUTERM_RESOURCES_DIR | path join 'shell-integration' 'nushell' 'huterm.nu')
            let failure = $'huterm: shell integration failed to load: ($body)'
            if (nu-check $body) {
                # A hook string permits dynamic source paths. Remove this one-shot
                # hook before loading, so any runtime error is reported once.
                let quoted = ($body | to nuon)
                let message = ($failure | to nuon)
                # Native cwd output precedes pre_prompt. Suppress that first
                # report, then let the body read the original option at load.
                let restore_osc7 = if $env.config.shell_integration.osc7 {
                    $env.config.shell_integration.osc7 = false
                    '$env.config.shell_integration.osc7 = true'
                } else { '' }
                let once = '# huterm-once
$env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | where {|hook| if ($hook | describe) == "string" { not ($hook | str starts-with "# huterm-once") } else { true } })'
                let snapshot = 'let huterm_before_prompt = $env.config.hooks.pre_prompt
let huterm_before_change = $env.config.hooks.env_change
let huterm_before_osc7 = $env.config.shell_integration.osc7
let huterm_before_path = $env.PATH'
                let hook = $once + (char nl) + $restore_osc7 + (char nl) + $snapshot + (char nl) + $'try { source ($quoted) } catch {
$env.config.hooks.pre_prompt = $huterm_before_prompt
$env.config.hooks.env_change = $huterm_before_change
$env.config.shell_integration.osc7 = $huterm_before_osc7
$env.PATH = $huterm_before_path
print -e ($message)
}'
                $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append $hook)
            } else {
                print -e $failure
            }
        }
    }
}
