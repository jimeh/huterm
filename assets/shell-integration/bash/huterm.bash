# Huterm Bash integration. See the provenance README for shell documentation.
[[ $- == *i* && ${_huterm_bash_loaded-} != "$$" ]] || return 0
_huterm_bash_loaded=$$
_huterm_bash_disabled=0
_huterm_bash_running=
_huterm_bash_skip=0
_huterm_bash_ps0=0
_huterm_bash_cmark=
_huterm_bash_features=,${HUTERM_SHELL_FEATURES-},
_huterm_bash_body_path=$HUTERM_RESOURCES_DIR/shell-integration/bash/huterm.bash

_huterm_bash_disable() {
    _huterm_bash_disabled=1
    _huterm_bash_cmark=
    _huterm_bash_running=
    PS0=${PS0%'${_huterm_bash_running:=$_huterm_bash_cmark}'}
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
    [[ $_huterm_bash_disabled == 0 && $_huterm_bash_ps0 == 0 ]] || return 0
    _huterm_bash_running=1
    if [[ $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;C\a' || _huterm_bash_failed
    fi
}
_huterm_bash_precmd() {
    local code=$_huterm_bash_status
    [[ $_huterm_bash_disabled == 0 ]] || return 0
    PS1=${PS1%$'\\[\e]133;B\a\\]'}
    if [[ -n $_huterm_bash_running && $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;D;%s\a' "$code" || { _huterm_bash_failed; return; }
    fi
    _huterm_bash_running=
    _huterm_bash_skip=0
    case ${PS1-}${PS2-} in
        *$'\e]133;'*|*'\e]133;'*|*'\033]133;'*|*'\x1b]133;'*) _huterm_bash_skip=1 ;;
    esac
    _huterm_bash_cwd || { _huterm_bash_failed; return; }
    # Feature: path. Delay until all startup files have completed.
    if [[ $_huterm_bash_features == *,path,* && -n ${HUTERM_BIN_DIR-} ]]; then
        case :$PATH: in
            *:"$HUTERM_BIN_DIR":*) ;;
            *) export PATH=$PATH:$HUTERM_BIN_DIR ;;
        esac
    fi
    _huterm_bash_cmark=
    if [[ $_huterm_bash_skip == 0 ]]; then
        _huterm_bash_cmark=$'\e]133;C\a'
        printf '\e]133;A\a' || { _huterm_bash_failed; return; }
        PS1=${PS1}$'\\[\e]133;B\a\\]'
    fi
    return 0
}
# Capture status before user PROMPT_COMMAND entries, render after them. Bash
# preserves the command status across a DEBUG trap. Never consult history: an
# ignorespace command still needs a C marker, without changing history policy.
# Modern Bash emits C through PS0; DEBUG supplies the 3.2 fallback and invokes
# any previous DEBUG trap once per command on both paths.
_huterm_bash_capture() {
    _huterm_bash_status=$?
    _huterm_bash_armed=0
    return "$_huterm_bash_status"
}
_huterm_bash_debug() {
    if [[ $_huterm_bash_armed == 1 && $BASH_COMMAND != _huterm_bash_capture && ${BASH_SUBSHELL-0} == 0 ]]; then
        _huterm_bash_armed=0
        _huterm_bash_preexec
        [[ -z $_huterm_bash_prior_debug ]] || eval "$_huterm_bash_prior_debug"
    fi
    return 0
}
_huterm_bash_arm() { _huterm_bash_armed=1; }
_huterm_bash_array_prompt() {
    # Older Bash versions execute only element zero, even if the user assigned
    # an array. Keep their scalar behavior when wrapping the prompt command.
    (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) ))
}
_huterm_bash_armed=0
# Install at the first prompt, outside the source scope that hides DEBUG traps.
# The trace attribute exposes a user's existing trap to this installer on 3.2.
_huterm_bash_install() {
    local code=$?
    _huterm_bash_prior_debug=$(trap -p DEBUG)
    if [[ -n $_huterm_bash_prior_debug ]]; then
        eval "_huterm_bash_trap_words=($_huterm_bash_prior_debug)"
        _huterm_bash_prior_debug=${_huterm_bash_trap_words[2]}
        unset _huterm_bash_trap_words
    fi
    trap '_huterm_bash_debug' DEBUG
    if _huterm_bash_array_prompt; then
        PROMPT_COMMAND[0]=_huterm_bash_capture
    else
        PROMPT_COMMAND=_huterm_bash_capture${PROMPT_COMMAND#_huterm_bash_install}
    fi
    _huterm_bash_status=$code
    _huterm_bash_armed=0
    return "$code"
}
# PS0 expands in the current shell. Assigning the marker records that a command
# was read and emits C in one expansion, including for a whole subshell command.
# The empty marker opts out when a prompt framework owns OSC 133.
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
    _huterm_bash_ps0=1
    PS0=${PS0-}'${_huterm_bash_running:=$_huterm_bash_cmark}'
fi
declare -ft _huterm_bash_install
if _huterm_bash_array_prompt; then
    PROMPT_COMMAND=(_huterm_bash_install "${PROMPT_COMMAND[@]}" _huterm_bash_precmd _huterm_bash_arm)
else
    PROMPT_COMMAND='_huterm_bash_install'${PROMPT_COMMAND:+$'\n'$PROMPT_COMMAND}$'\n_huterm_bash_precmd\n_huterm_bash_arm'
fi
