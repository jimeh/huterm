# Huterm Nushell bootstrap. Vendor autoload runs after the user's startup files.
module huterm {
    export-env {
        if ($env.HUTERM_SHELL_INTEGRATION? | default '') == 'nu' {
            hide-env HUTERM_SHELL_INTEGRATION
            let original = ($env.HUTERM_ORIG_XDG_DATA_DIRS? | default '0')
            if ($original | str starts-with '1') {
                $env.XDG_DATA_DIRS = ($original | str substring 1..)
            } else {
                hide-env -i XDG_DATA_DIRS
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
                let hook = $once + (char nl) + $restore_osc7 + (char nl) + $'try { source ($quoted) } catch { print -e ($message) }'
                $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append $hook)
            } else {
                print -e $failure
            }
        }
    }
}
