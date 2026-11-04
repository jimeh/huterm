# Huterm shell integration bootstrap. Written from the Bash startup-file manual.
# Keep the startup files before the optional integration body.
[[ ${HUTERM_SHELL_INTEGRATION-} == bash ]] || return 0
unset HUTERM_SHELL_INTEGRATION
_huterm_bash_flags=${HUTERM_BASH_INJECT-}
_huterm_bash_login=${HUTERM_BASH_LOGIN-}
_huterm_bash_rcfile=${HUTERM_BASH_RCFILE-$HOME/.bashrc}
unset HUTERM_BASH_INJECT HUTERM_BASH_LOGIN HUTERM_BASH_RCFILE
# Core starts a requested login shell as an interactive one with this file as
# its rcfile, so the first branch emulates the login sequence. Each branch ends
# with the last startup file it sources, so the next statement sees that
# file's status, which the first prompt shows as in a plain start.
if [[ $_huterm_bash_login == 1 ]]; then
    if [[ $_huterm_bash_flags != *p* ]]; then
        if [[ -r /etc/profile ]]; then
            source /etc/profile
        fi
        if [[ -r $HOME/.bash_profile ]]; then
            source "$HOME/.bash_profile"
        elif [[ -r $HOME/.bash_login ]]; then
            source "$HOME/.bash_login"
        elif [[ -r $HOME/.profile ]]; then
            source "$HOME/.profile"
        fi
    fi
elif [[ -r $_huterm_bash_rcfile ]]; then
    source "$_huterm_bash_rcfile"
fi
# A user's DEBUG trap sees only these two statements: the first saves the
# status, $_, and the trap, and the second sets the trap aside for Huterm's
# own. Sourcing has already replaced $_ and PIPESTATUS from the file's last
# command, which a plain start would show at the first prompt.
_huterm_bash_last=$_ _huterm_bash_saved_debug=$(printf '%s:' "$?"; trap -p DEBUG)
trap - DEBUG
_huterm_bash_startup_status=${_huterm_bash_saved_debug%%:*}
_huterm_bash_saved_debug=${_huterm_bash_saved_debug#*:}
unset _huterm_bash_flags _huterm_bash_login _huterm_bash_rcfile
_huterm_bash_body=${HUTERM_RESOURCES_DIR-}/shell-integration/bash/huterm.bash
if ( "$BASH" -n "$_huterm_bash_body" ) 2>/dev/null && source "$_huterm_bash_body"; then
    :
else
    declare -F _huterm_bash_disable >/dev/null && _huterm_bash_disable
    printf 'huterm: shell integration failed to load: %s\n' "$_huterm_bash_body" >&2
fi
unset _huterm_bash_body
# The body's trap runs the user's trap. Install it last, at shell scope, which
# Bash 3.2 requires, so the user's trap does not see Huterm's statements.
if [[ -n ${_huterm_bash_trap-} ]]; then
    trap -- "$_huterm_bash_trap" DEBUG
fi
_huterm_bash_finish() {
    # Bash 4.4 and later, and Bash 3.2 without a trap, keep a DEBUG trap set in
    # a function.
    if [[ -z ${_huterm_bash_trap-} ]]; then
        eval "$_huterm_bash_saved_debug"
    fi
    unset -f _huterm_bash_finish
    unset _huterm_bash_saved_debug _huterm_bash_startup_status _huterm_bash_trap _huterm_bash_last
    return "$1"
}
# In an && list a nonzero startup status does not count as a failed command,
# so an ERR trap or errexit does not fire on Huterm's restore. `: "$_"` keeps
# the restored last argument when the status is 0.
_huterm_bash_finish "$_huterm_bash_startup_status" "$_huterm_bash_last" && : "$_"
