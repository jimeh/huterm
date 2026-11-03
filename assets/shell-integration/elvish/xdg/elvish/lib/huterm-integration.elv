# Huterm Elvish module bootstrap. Elvish caches modules once per process.
use str
if (has-env HUTERM_SHELL_INTEGRATION) {
    unset-env HUTERM_SHELL_INTEGRATION
}
if (has-env HUTERM_ORIG_XDG_DATA_DIRS) {
    var original = $E:HUTERM_ORIG_XDG_DATA_DIRS
    if (str:has-prefix $original 1) {
        set-env XDG_DATA_DIRS (str:trim-prefix $original 1)
    } else {
        unset-env XDG_DATA_DIRS
    }
    unset-env HUTERM_ORIG_XDG_DATA_DIRS
}
if (has-env HUTERM_XDG_ENTRY) { unset-env HUTERM_XDG_ENTRY }
var before-readline = $edit:before-readline
var after-readline = $edit:after-readline
var after-command = $edit:after-command
try {
    eval (slurp < $E:HUTERM_RESOURCES_DIR/shell-integration/elvish/huterm.elv)
} catch {
    set edit:before-readline = $before-readline
    set edit:after-readline = $after-readline
    set edit:after-command = $after-command
    echo 'huterm: shell integration failed to load: '$E:HUTERM_RESOURCES_DIR'/shell-integration/elvish/huterm.elv' >&2
}
