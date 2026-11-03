# Huterm Bash integration. See the provenance README for shell documentation.
[[ $- == *i* && ${_huterm_bash_loaded-} != "$$" ]] || return 0
_huterm_bash_loaded=$$
_huterm_bash_disabled=0
_huterm_bash_running=0
_huterm_bash_skip=0
_huterm_bash_features=,${HUTERM_SHELL_FEATURES-},
_huterm_bash_body_path=$HUTERM_RESOURCES_DIR/shell-integration/bash/huterm.bash

_huterm_bash_disable() {
    _huterm_bash_disabled=1
    local index
    for index in "${!preexec_functions[@]}"; do
        [[ ${preexec_functions[index]} != _huterm_bash_preexec ]] || unset 'preexec_functions[index]'
    done
    for index in "${!precmd_functions[@]}"; do
        [[ ${precmd_functions[index]} != _huterm_bash_precmd ]] || unset 'precmd_functions[index]'
    done
    PS1=${PS1%$'\\[\e]133;B\a\\]'}
}
_huterm_bash_failed() {
    _huterm_bash_disable
    printf 'huterm: shell integration failed to load: %s\n' "$_huterm_bash_body_path" >&2
}
_huterm_bash_cwd() {
    local LC_ALL=C host=${HOSTNAME-}
    [[ $host$PWD != *[[:cntrl:]]* && $host$PWD != *$'\302'[$'\200'-$'\237']* ]] || return 0
    printf '\e]7;kitty-shell-cwd://%s%s\a' "$host" "$PWD"
}
_huterm_bash_preexec() {
    [[ $_huterm_bash_disabled == 0 ]] || return 0
    _huterm_bash_running=1
    if [[ $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;C\a' || _huterm_bash_failed
    fi
}
_huterm_bash_precmd() {
    local code=$?
    [[ $_huterm_bash_disabled == 0 ]] || return 0
    PS1=${PS1%$'\\[\e]133;B\a\\]'}
    _huterm_bash_skip=0
    case ${PS1-}${PS2-} in
        *$'\e]133;'*|*'\e]133;'*|*'\033]133;'*|*'\x1b]133;'*) _huterm_bash_skip=1 ;;
    esac
    if [[ $_huterm_bash_running == 1 && $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;D;%s\a' "$code" || { _huterm_bash_failed; return; }
    fi
    _huterm_bash_running=0
    _huterm_bash_cwd || { _huterm_bash_failed; return; }
    # Feature: path. Delay until all startup files have completed.
    if [[ $_huterm_bash_features == *,path,* && -n ${HUTERM_BIN_DIR-} ]]; then
        case :$PATH: in
            *:"$HUTERM_BIN_DIR":*) ;;
            *) export PATH=$PATH:$HUTERM_BIN_DIR ;;
        esac
    fi
    if [[ $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;A\a' || { _huterm_bash_failed; return; }
        PS1=${PS1}$'\\[\e]133;B\a\\]'
    fi
    return 0
}
# bash-preexec owns the DEBUG trap and preserves existing prompt commands.
source "$HUTERM_RESOURCES_DIR/shell-integration/bash/bash-preexec.sh" || return 1
preexec_functions+=(_huterm_bash_preexec)
precmd_functions+=(_huterm_bash_precmd)
