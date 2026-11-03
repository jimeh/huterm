# Huterm Nushell integration. Native osc133 provides command markers.
# A never-run hook record is a process-local guard. Unlike an environment
# variable it cannot leak into nested shells, and repeated source is a no-op.
let loaded = ($env.config.hooks.pre_prompt | any {|hook|
    try { $hook.code == '# huterm-loaded' } catch { false }
})
if not $loaded {
    # Native OSC 7 does not encode URL delimiters. Replace it only when enabled
    # at load, preserving the user's opt-out and leaving native OSC 133 alone.
    if $env.config.shell_integration.osc7 {
        $env.config.shell_integration.osc7 = false
        let failure = $'huterm: shell integration failed to load: ($env.HUTERM_RESOURCES_DIR? | default "")/shell-integration/nushell/huterm.nu'
        let cwd = {||
            try {
                let host = (sys host | get hostname)
                if not (($host + $env.PWD) =~ '[\x00-\x1f\x7f-\x9f]') {
                    print -n $'(char --integer 27)]7;kitty-shell-cwd://($host)($env.PWD)(char bel)'
                }
            } catch {
                print -e $failure
                $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append {
                    condition: {|| false }, code: '# huterm-cwd-disabled'
                })
            }
        }
        let hook = {
            condition: {|| not ($env.config.hooks.pre_prompt | any {|hook|
                try { $hook.code == '# huterm-cwd-disabled' } catch { false }
            }) },
            code: $cwd
        }
        $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append $hook)
        $env.config.hooks.env_change.PWD = ($env.config.hooks.env_change.PWD? | default [] | append $hook)
        do $cwd
    }
    let features = ($env.HUTERM_SHELL_FEATURES? | default '' | split row ',')
    # Feature: path. This runs after the user's startup files.
    if ('path' in $features) and ($env.HUTERM_BIN_DIR? != null) {
        if $env.HUTERM_BIN_DIR not-in $env.PATH {
            $env.PATH = ($env.PATH | append $env.HUTERM_BIN_DIR)
        }
    }
    $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | append {
        condition: {|| false }, code: '# huterm-loaded'
    })
}
