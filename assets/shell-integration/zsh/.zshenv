# Huterm Zsh bootstrap. Restore ZDOTDIR before native startup continues.
if [[ ${HUTERM_SHELL_INTEGRATION-} == zsh ]]; then
    unset HUTERM_SHELL_INTEGRATION
    if [[ ${HUTERM_ORIG_ZDOTDIR-} == 1* ]]; then
        export ZDOTDIR=${HUTERM_ORIG_ZDOTDIR#?}
    else
        unset ZDOTDIR
    fi
    unset HUTERM_ORIG_ZDOTDIR
    [[ ! -r ${ZDOTDIR-$HOME}/.zshenv ]] || source "${ZDOTDIR-$HOME}/.zshenv"
    if [[ -o interactive ]]; then
        _huterm_zsh_body=${HUTERM_RESOURCES_DIR-}/shell-integration/zsh/huterm.zsh
        {
            # Parse a function definition without executing it or finding zsh
            # on PATH. Strict syntax rejects incomplete short-loop constructs.
            if (
                unsetopt shortloops
                _huterm_zsh_source=$(/bin/cat -- "$_huterm_zsh_body") || exit
                eval "_huterm_zsh_load() { $_huterm_zsh_source
}"
            ) 2>/dev/null; then
                source "$_huterm_zsh_body"
            else
                false
            fi
        } always {
            _huterm_zsh_result=$?
            if (( TRY_BLOCK_ERROR || _huterm_zsh_result != 0 )); then
                (( ${+functions[_huterm_zsh_disable]} )) && _huterm_zsh_disable
                print -u2 -r -- "huterm: shell integration failed to load: $_huterm_zsh_body"
                TRY_BLOCK_ERROR=0
            fi
            unset _huterm_zsh_body _huterm_zsh_result
        }
    fi
fi
