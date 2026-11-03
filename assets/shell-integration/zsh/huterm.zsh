# Huterm Zsh integration. Uses documented precmd, preexec, and chpwd hooks.
[[ -o interactive && ${_huterm_zsh_loaded-} != $$ ]] || return 0
typeset -g _huterm_zsh_loaded=$$ _huterm_zsh_disabled=0 _huterm_zsh_running=0 _huterm_zsh_skip=0
typeset -g _huterm_zsh_features=,${HUTERM_SHELL_FEATURES-},
typeset -g _huterm_zsh_body_path=$HUTERM_RESOURCES_DIR/shell-integration/zsh/huterm.zsh
_huterm_zsh_disable() {
    _huterm_zsh_disabled=1
    precmd_functions=(${precmd_functions:#_huterm_zsh_precmd})
    preexec_functions=(${preexec_functions:#_huterm_zsh_preexec})
    chpwd_functions=(${chpwd_functions:#_huterm_zsh_cwd})
    PROMPT=${PROMPT%$'%{\e]133;B\a%}'}
}
_huterm_zsh_failed() {
    _huterm_zsh_disable
    print -u2 -r -- "huterm: shell integration failed to load: $_huterm_zsh_body_path"
}
_huterm_zsh_cwd() {
    emulate -L zsh
    local LC_ALL=C
    [[ $HOST$PWD != *[[:cntrl:]]* && $HOST$PWD != *$'\302'[$'\200'-$'\237']* ]] || return 0
    printf '\e]7;kitty-shell-cwd://%s%s\a' "$HOST" "$PWD" || _huterm_zsh_failed
}
_huterm_zsh_preexec() {
    emulate -L zsh
    [[ $_huterm_zsh_disabled == 0 ]] || return 0
    _huterm_zsh_running=1
    if [[ $_huterm_zsh_skip == 0 ]]; then
        printf '\e]133;C\a' || _huterm_zsh_failed
    fi
}
_huterm_zsh_precmd() {
    local code=$?
    emulate -L zsh
    [[ $_huterm_zsh_disabled == 0 ]] || return 0
    PROMPT=${PROMPT%$'%{\e]133;B\a%}'}
    _huterm_zsh_skip=0
    case ${PROMPT-}${PS2-} in
        *$'\e]133;'*|*'\e]133;'*|*'\033]133;'*|*'\x1b]133;'*) _huterm_zsh_skip=1 ;;
    esac
    if [[ $_huterm_zsh_running == 1 && $_huterm_zsh_skip == 0 ]]; then
        printf '\e]133;D;%s\a' "$code" || { _huterm_zsh_failed; return; }
    fi
    _huterm_zsh_running=0
    _huterm_zsh_cwd
    # Feature: path. Native startup files run before the first precmd.
    if [[ $_huterm_zsh_features == *,path,* && -n ${HUTERM_BIN_DIR-} ]] && (( ! ${path[(Ie)$HUTERM_BIN_DIR]} )); then
        export PATH=$PATH:$HUTERM_BIN_DIR
    fi
    if [[ $_huterm_zsh_skip == 0 ]]; then
        printf '\e]133;A\a' || { _huterm_zsh_failed; return; }
        PROMPT+=$'%{\e]133;B\a%}'
    fi
    return 0
}
typeset -ga precmd_functions preexec_functions chpwd_functions
precmd_functions+=(_huterm_zsh_precmd)
preexec_functions+=(_huterm_zsh_preexec)
chpwd_functions+=(_huterm_zsh_cwd)
