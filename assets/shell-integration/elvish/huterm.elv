# Huterm Elvish integration. Editor hooks run synchronously outside prompts.
use re
use str
var features = []
if (has-env HUTERM_SHELL_FEATURES) { set features = [(str:split , $E:HUTERM_SHELL_FEATURES)] }
var disabled = $false
var running = $false
fn failed {
    if (not $disabled) {
        set disabled = $true
        echo 'huterm: shell integration failed to load: '$E:HUTERM_RESOURCES_DIR'/shell-integration/elvish/huterm.elv' >&2
    }
}
fn cwd {
    var host = (hostname)
    if (not (re:match '[\x00-\x1f\x7f-\x9f]' $host$pwd)) {
        printf "\x1b]7;kitty-shell-cwd://%s%s\x07" $host $pwd
    }
}
set edit:before-readline = [$@edit:before-readline {
    if (not $disabled) {
        try {
            # Feature: path. rc.elv has completed before the first editor cycle.
            if (and (has-value $features path) (has-env HUTERM_BIN_DIR)) {
                if (not (has-value $paths $E:HUTERM_BIN_DIR)) {
                    set paths = [$@paths $E:HUTERM_BIN_DIR]
                }
            }
            cwd
            printf "\x1b]133;A\x07"
        } catch { failed }
    }
}]
set edit:after-readline = [$@edit:after-readline {|line|
    if (and (not $disabled) (not (eq $line ''))) {
        try { set running = $true; printf "\x1b]133;C\x07" } catch { failed }
    }
}]
set edit:after-command = [$@edit:after-command {|info|
    if (and (not $disabled) $running) {
        try {
            var code = 0
            if (not (eq $info[error] $nil)) {
                set code = 1
                try { set code = $info[error][reason][exit-status] } catch { }
            }
            printf "\x1b]133;D;%s\x07" $code
            set running = $false
        } catch { failed }
    }
}]
