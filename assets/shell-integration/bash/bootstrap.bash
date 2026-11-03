# Huterm shell integration bootstrap. Written from the Bash startup-file manual.
# Keep startup restoration before the optional integration body.
[[ ${HUTERM_SHELL_INTEGRATION-} == bash ]] || return 0
unset HUTERM_SHELL_INTEGRATION
_huterm_bash_flags=${HUTERM_BASH_INJECT-}
_huterm_bash_login=${HUTERM_BASH_LOGIN-}
_huterm_bash_rcfile=${HUTERM_BASH_RCFILE-}
unset HUTERM_BASH_INJECT HUTERM_BASH_LOGIN HUTERM_BASH_RCFILE
if [[ ${HUTERM_ORIG_ENV-} == 1* ]]; then
    export ENV=${HUTERM_ORIG_ENV#?}
else
    unset ENV
fi
if [[ ${HUTERM_ORIG_HISTFILE-} == 1* ]]; then
    export HISTFILE=${HUTERM_ORIG_HISTFILE#?}
else
    unset HISTFILE
fi
unset HUTERM_ORIG_ENV HUTERM_ORIG_HISTFILE
set +o posix
if [[ $_huterm_bash_login == 1 ]] || shopt -q login_shell; then
    if [[ $_huterm_bash_flags != *p* ]]; then
        [[ ! -r /etc/profile ]] || source /etc/profile
        for _huterm_bash_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
            if [[ -r $_huterm_bash_profile ]]; then
                source "$_huterm_bash_profile"
                break
            fi
        done
    fi
elif [[ $_huterm_bash_flags != *r* ]]; then
    _huterm_bash_rcfile=${_huterm_bash_rcfile:-$HOME/.bashrc}
    [[ ! -r $_huterm_bash_rcfile ]] || source "$_huterm_bash_rcfile"
fi
unset _huterm_bash_flags _huterm_bash_login _huterm_bash_rcfile _huterm_bash_profile
_huterm_bash_body=$HUTERM_RESOURCES_DIR/shell-integration/bash/huterm.bash
if ( "$BASH" -n "$_huterm_bash_body" ) 2>/dev/null && source "$_huterm_bash_body"; then
    :
else
    declare -F _huterm_bash_disable >/dev/null && _huterm_bash_disable
    printf 'huterm: shell integration failed to load: %s\n' "$_huterm_bash_body" >&2
fi
unset _huterm_bash_body
