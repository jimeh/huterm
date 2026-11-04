# Huterm Zsh integration. Uses documented precmd, preexec, and chpwd hooks.
[[ -o interactive && ${_huterm_zsh_loaded-} != $$ ]] || return 0
typeset -g _huterm_zsh_loaded=$$ _huterm_zsh_disabled=0 _huterm_zsh_running=0
typeset -g _huterm_zsh_marked=0 _huterm_zsh_reported=0 _huterm_zsh_scheduled=0
typeset -g _huterm_zsh_features=,${HUTERM_SHELL_FEATURES-},
# The exact prompt Huterm last set and the prompt it decorated. Huterm puts
# that prompt back only while PROMPT still equals its own value, so it never
# removes marks it did not add or replaces a prompt the user changed.
typeset -g _huterm_zsh_owned=0 _huterm_zsh_prompt_set= _huterm_zsh_prompt_user=
typeset -g _huterm_zsh_body_path=${HUTERM_RESOURCES_DIR-}/shell-integration/zsh/huterm.zsh
typeset -g _huterm_zsh_amark=$'%{\e]133;A\e\\%}' _huterm_zsh_bmark=$'%{\e]133;B\e\\%}'
# A late decoration runs as a scheduled command. Its arguments keep $_, and
# the && keeps a non-zero $? from triggering ZERR or ERR_EXIT.
typeset -g _huterm_zsh_late_command='_huterm_zsh_late "$_" && _huterm_zsh_keep "$_"'
typeset -ga _huterm_zsh_c1
() {
    emulate -L zsh
    # The 32 UTF-8 encodings of C1 controls, compared as literal substrings so
    # the check does not depend on the locale or on multibyte mode.
    local x y sequence
    _huterm_zsh_c1=()
    for x in 0 1 2 3; do
        for y in 0 1 2 3 4 5 6 7; do
            printf -v sequence "\\302\\2$x$y"
            _huterm_zsh_c1+=("$sequence")
        done
    done
}
_huterm_zsh_restore() {
    if (( _huterm_zsh_owned )) && [[ $PROMPT == "$_huterm_zsh_prompt_set" ]]; then
        PROMPT=$_huterm_zsh_prompt_user
    fi
    _huterm_zsh_owned=0
}
_huterm_zsh_unschedule() {
    emulate -L zsh
    local index
    (( _huterm_zsh_scheduled )) || return 0
    _huterm_zsh_scheduled=0
    (( $+zsh_scheduled_events )) || return 0
    for (( index = 1; index <= $#zsh_scheduled_events; index++ )); do
        if [[ ${zsh_scheduled_events[index]#*:*:} == "$_huterm_zsh_late_command" ]]; then
            sched -$index
            return 0
        fi
    done
}
_huterm_zsh_schedule() {
    emulate -L zsh
    (( ! _huterm_zsh_scheduled && $+builtins[sched] )) || return 0
    sched +0 "$_huterm_zsh_late_command" && _huterm_zsh_scheduled=1
}
_huterm_zsh_disable() {
    emulate -L zsh
    _huterm_zsh_disabled=1
    precmd_functions=(${precmd_functions:#_huterm_zsh_*})
    preexec_functions=(${preexec_functions:#_huterm_zsh_*})
    chpwd_functions=(${chpwd_functions:#_huterm_zsh_*})
    _huterm_zsh_restore
    _huterm_zsh_unschedule
}
_huterm_zsh_failed() {
    _huterm_zsh_disable
    print -u2 -r -- "huterm: shell integration failed to load: $_huterm_zsh_body_path"
}
# Keep the status hook first, so D precedes output from other precmd hooks,
# and the decoration hook last, so it wraps the prompt they build. A startup
# file that assigned the hook arrays gets Huterm's hooks back here.
_huterm_zsh_install() {
    emulate -L zsh
    typeset -ga precmd_functions preexec_functions chpwd_functions
    precmd_functions=(_huterm_zsh_status ${precmd_functions:#_huterm_zsh_(status|precmd)} _huterm_zsh_precmd)
    preexec_functions=(${preexec_functions:#_huterm_zsh_preexec} _huterm_zsh_preexec)
    chpwd_functions=(${chpwd_functions:#_huterm_zsh_cwd} _huterm_zsh_cwd)
}
_huterm_zsh_cwd() {
    emulate -L zsh
    local value=$HOST$PWD sequence
    [[ $value != *[[:cntrl:]]* ]] || return 0
    if [[ $value == *$'\302'* ]]; then
        for sequence in $_huterm_zsh_c1; do
            [[ $value != *"$sequence"* ]] || return 0
        done
    fi
    printf '\e]7;kitty-shell-cwd://%s%s\a' "$HOST" "$PWD" || _huterm_zsh_failed
}
_huterm_zsh_report() {
    _huterm_zsh_reported=1
    _huterm_zsh_cwd
    # Feature: path. Native startup files run before the first precmd.
    if [[ $_huterm_zsh_features == *,path,* && -n ${HUTERM_BIN_DIR-} ]] && (( ! ${path[(Ie)$HUTERM_BIN_DIR]} )); then
        export PATH=$PATH:$HUTERM_BIN_DIR
    fi
}
_huterm_zsh_status() {
    local code=$?
    emulate -L zsh
    (( ! _huterm_zsh_disabled )) || return 0
    if (( _huterm_zsh_running )); then
        _huterm_zsh_running=0
        printf '\e]133;D;%s\e\\' "$code" || { _huterm_zsh_failed; return 0; }
    fi
    _huterm_zsh_report
}
# $1 is 1 when the user's prompt_percent option is set. Without it, zsh cannot
# mark text as zero-width, so A is printed directly and B is omitted.
_huterm_zsh_decorate() {
    emulate -L zsh
    _huterm_zsh_restore
    (( _huterm_zsh_reported )) || _huterm_zsh_report
    _huterm_zsh_reported=0
    _huterm_zsh_marked=0
    case $PROMPT$PS2 in
        (*$'\e]133;'*|*'\e]133;'*|*'\E]133;'*|*'\033]133;'*|*'\x1b]133;'*) return 0 ;;
    esac
    _huterm_zsh_marked=1
    if (( $1 )); then
        _huterm_zsh_prompt_user=$PROMPT
        PROMPT=$_huterm_zsh_amark$PROMPT$_huterm_zsh_bmark
        _huterm_zsh_prompt_set=$PROMPT
        _huterm_zsh_owned=1
    else
        printf '\e]133;A\e\\' || _huterm_zsh_failed
    fi
}
_huterm_zsh_precmd() {
    local percent=0
    [[ -o prompt_percent ]] && percent=1
    emulate -L zsh
    (( ! _huterm_zsh_disabled )) || return 0
    if [[ ${precmd_functions[-1]-} == _huterm_zsh_precmd ]]; then
        _huterm_zsh_unschedule
        _huterm_zsh_decorate $percent
    else
        # Hooks after this one may rebuild the prompt. A scheduled command runs
        # after every precmd hook, before the single prompt expansion.
        _huterm_zsh_install
        _huterm_zsh_schedule
    fi
}
# The scheduled decoration. Its return value keeps $? for the prompt and the
# next command; a pipeline status reduces to that value.
_huterm_zsh_late() {
    local code=$? percent=0
    [[ -o prompt_percent ]] && percent=1
    emulate -L zsh
    _huterm_zsh_scheduled=0
    if (( ! _huterm_zsh_disabled )); then
        _huterm_zsh_install
        _huterm_zsh_decorate $percent
    fi
    return $code
}
_huterm_zsh_keep() { return 0; }
_huterm_zsh_preexec() {
    emulate -L zsh
    (( ! _huterm_zsh_disabled )) || return 0
    # Commands see the user's own prompt, as in a plain shell.
    _huterm_zsh_restore
    _huterm_zsh_install
    if (( _huterm_zsh_marked )); then
        _huterm_zsh_running=1
        printf '\e]133;C\e\\' || _huterm_zsh_failed
    fi
}
_huterm_zsh_install
# Startup files that run after this one may add prompt hooks or replace the
# hook arrays. The first prompt is decorated after all of them.
zmodload zsh/sched 2>/dev/null && _huterm_zsh_schedule
return 0
