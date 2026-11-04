# Huterm Bash integration. See the provenance README for shell documentation.
[[ $- == *i* && ${_huterm_bash_loaded-} != "$$" ]] || return 0
_huterm_bash_loaded=$$
_huterm_bash_disabled=0
_huterm_bash_running=0
_huterm_bash_skip=0
_huterm_bash_ps0=0
_huterm_bash_armed=0
_huterm_bash_number='\#'
_huterm_bash_first_prompt=1
_huterm_bash_features=,${HUTERM_SHELL_FEATURES-},
_huterm_bash_body_path=$HUTERM_RESOURCES_DIR/shell-integration/bash/huterm.bash
_huterm_bash_cmark='\e]133;C\a'

_huterm_bash_disable() {
    _huterm_bash_disabled=1
    _huterm_bash_running=0
    PS0=${PS0%"$_huterm_bash_cmark"}
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
    if [[ $_huterm_bash_ps0 == 0 && $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;C\a' || _huterm_bash_failed
    fi
    return 0
}
_huterm_bash_precmd() {
    local code=$_huterm_bash_status
    [[ $_huterm_bash_disabled == 0 ]] || return 0
    PS1=${PS1%$'\\[\e]133;B\a\\]'}
    PS0=${PS0%"$_huterm_bash_cmark"}
    if [[ $_huterm_bash_running == 1 && $_huterm_bash_skip == 0 ]]; then
        printf '\e]133;D;%s\a' "$code" || { _huterm_bash_failed; return; }
    fi
    _huterm_bash_running=0
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
    if [[ $_huterm_bash_skip == 0 ]]; then
        [[ $_huterm_bash_ps0 == 0 ]] || PS0=${PS0-}$_huterm_bash_cmark
        printf '\e]133;A\a' || { _huterm_bash_failed; return; }
        PS1=${PS1}$'\\[\e]133;B\a\\]'
    fi
    _huterm_bash_armed=1
    return 0
}
_huterm_bash_capture() {
    _huterm_bash_status=$?
    _huterm_bash_armed=0
    _huterm_bash_record_number
    return "$_huterm_bash_status"
}
_huterm_bash_record_number() {
    if [[ $_huterm_bash_ps0 == 1 ]]; then
        local number=${_huterm_bash_number@P}
        if [[ $_huterm_bash_first_prompt == 1 ]]; then
            _huterm_bash_first_prompt=0
            _huterm_bash_previous_number=$((number - 1))
        else
            [[ $number == "$_huterm_bash_previous_number" ]] || _huterm_bash_running=1
            _huterm_bash_previous_number=$number
        fi
    fi
    return 0
}
_huterm_bash_bp_precmd() {
    _huterm_bash_status=$?
    _huterm_bash_record_number
}
_huterm_bash_bp_prompt() {
    _huterm_bash_precmd
    # Older bash-preexec versions arm before the remaining PROMPT_COMMAND
    # entries. Re-arm through their own function after final decoration.
    if declare -F __bp_interactive_mode >/dev/null; then
        __bp_interactive_mode force
    fi
}
# The original trap runs verbatim after its incoming status and last argument
# are restored, outside a function or eval. Its return value remains the trap
# result, preserving per-simple-command firing and extdebug return semantics.
_huterm_bash_debug() {
    _huterm_bash_debug_status=$1
    _huterm_bash_debug_argument=$2
    _huterm_bash_chain=0
    local frame
    if [[ -n $_huterm_bash_prior_debug ]]; then
        _huterm_bash_chain=1
        for frame in "${FUNCNAME[@]:1}"; do
            case $frame in _huterm_bash_*) _huterm_bash_chain=0; break ;; esac
        done
    fi
    if [[ $_huterm_bash_armed == 1 && $BASH_COMMAND != _huterm_bash_capture && ${BASH_SUBSHELL-0} == 0 ]]; then
        _huterm_bash_armed=0
        _huterm_bash_preexec
    fi
    return 0
}
_huterm_bash_restore() { return "$1"; }
_huterm_bash_build_debug() {
    local words=()
    eval "words=($_huterm_bash_prior_debug)"
    _huterm_bash_prior_debug=${words[2]-}
    local body='_huterm_bash_debug "$?" "$_"; if [[ $_huterm_bash_chain == 1 ]]; then _huterm_bash_restore "$_huterm_bash_debug_status" "$_huterm_bash_debug_argument"; '
    body+=${_huterm_bash_prior_debug:-:}
    body+=$'\nelse : "$_huterm_bash_debug_argument"; fi'
    printf -v _huterm_bash_trap_command 'trap -- %q DEBUG' "$body"
}
_huterm_bash_array_prompt() {
    (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) || return 1
    [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]
}
# Only the first prompt uses the installer. Keep it after lazy prompt installers
# so their final PROMPT_COMMAND, DEBUG trap, and callback arrays are available.
_huterm_bash_install() {
    if [[ -n ${bash_preexec_imported-}${__bp_imported-} ]]; then
        preexec_functions+=(_huterm_bash_preexec)
        precmd_functions+=(_huterm_bash_bp_precmd)
        # Remove our temporary entries from the prompt command the lazy
        # installer retained. Leave its own invoker and trap in charge.
        local entry cleaned=()
        if _huterm_bash_array_prompt; then
            for entry in "${PROMPT_COMMAND[@]}"; do
                entry=${entry//"$_huterm_bash_initial_debug"/}
                entry=${entry//'_huterm_bash_capture'/}
                entry=${entry//'_huterm_bash_install'/}
                cleaned+=("$entry")
            done
            PROMPT_COMMAND=("${cleaned[@]}")
        else
            PROMPT_COMMAND=${PROMPT_COMMAND//"$_huterm_bash_initial_debug"/}
            PROMPT_COMMAND=${PROMPT_COMMAND//'_huterm_bash_capture'/}
            PROMPT_COMMAND=${PROMPT_COMMAND//'_huterm_bash_install'/}
        fi
        # Existing PROMPT_COMMAND themes run after bash-preexec's precmd
        # callbacks. Final decoration must follow those entries as well.
        if _huterm_bash_array_prompt; then
            PROMPT_COMMAND+=(_huterm_bash_bp_prompt)
        else
            PROMPT_COMMAND+=$'\n_huterm_bash_bp_prompt'
        fi
        _huterm_bash_bp_prompt
    else
        PROMPT_COMMAND=${PROMPT_COMMAND#"$_huterm_bash_initial_debug"}
        if _huterm_bash_array_prompt; then
            PROMPT_COMMAND[${#PROMPT_COMMAND[@]}-1]=_huterm_bash_precmd
        else
            PROMPT_COMMAND=${PROMPT_COMMAND%_huterm_bash_install}_huterm_bash_precmd
        fi
        _huterm_bash_precmd
    fi
}
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
    _huterm_bash_ps0=1
fi
_huterm_bash_initial_debug=
if [[ $_huterm_bash_ps0 == 0 ]]; then
    # Capture and replace the trap at shell scope. A traced installer function
    # would expose its own statements to a user's DEBUG trap on Bash 3.2.
    _huterm_bash_initial_debug='if [[ -z ${bash_preexec_imported-}${__bp_imported-} ]]; then
_huterm_bash_prior_debug=$(trap -p DEBUG)
trap - DEBUG
_huterm_bash_build_debug
eval "$_huterm_bash_trap_command"
fi
'
fi
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
    [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND=(_huterm_bash_capture "${PROMPT_COMMAND[@]}" _huterm_bash_install)
else
    PROMPT_COMMAND=${_huterm_bash_initial_debug}'_huterm_bash_capture'${PROMPT_COMMAND:+$'\n'$PROMPT_COMMAND}$'\n_huterm_bash_install'
fi
