# Huterm Bash integration. See the provenance README for references.
[[ $- == *i* && ${_huterm_bash_loaded-} != "$$" ]] || return 0
_huterm_bash_loaded=$$
_huterm_bash_disabled=0
_huterm_bash_body_path=${HUTERM_RESOURCES_DIR-}/shell-integration/bash/huterm.bash
_huterm_bash_features=,${HUTERM_SHELL_FEATURES-},
# Huterm writes the semicolons in its marks as \073. Bash decodes them to the
# same bytes, and the distinct text lets each decoration remove exactly
# Huterm's own marks, never a prompt framework's.
_huterm_bash_amark='\[\e]133\073A\a\]'
_huterm_bash_bmark='\[\e]133\073B\a\]'
# Prompt expansion runs after Bash restores the command's $? from before
# PROMPT_COMMAND, so this reports the exact status without a capture entry.
_huterm_bash_dmark='\[\e]133\073D\073$?\a\]'
_huterm_bash_cmark='\e]133\073C\a'
# With promptvars on, PS0's expansion also records that a command is about to
# run. The zero-length substring hides the arithmetic result.
_huterm_bash_ranmark='${_huterm_bash_cmark:0:$((_huterm_bash_ran=1,0))}'
_huterm_bash_capture_entry='_huterm_bash_capture "$_" && _huterm_bash_keep "$_"'
_huterm_bash_prompt_entry=_huterm_bash_prompt
_huterm_bash_number_escape='\#'
_huterm_bash_number=
_huterm_bash_marked=0
_huterm_bash_flagged=0
_huterm_bash_ran=0
_huterm_bash_running=0
_huterm_bash_armed=0
_huterm_bash_captured=0
_huterm_bash_status=0
_huterm_bash_bp_epilogue=0
_huterm_bash_ps0=0
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
    _huterm_bash_ps0=1
fi
_huterm_bash_ps0_unset=0
if [[ -z ${PS0+set} ]]; then
    _huterm_bash_ps0_unset=1
fi
# The 32 UTF-8 encodings of C1 controls, compared as literal substrings so the
# check does not depend on the locale.
_huterm_bash_c1=()
for _huterm_bash_x in 0 1 2 3; do
    for _huterm_bash_y in 0 1 2 3 4 5 6 7; do
        printf -v _huterm_bash_z "\\302\\2$_huterm_bash_x$_huterm_bash_y"
        _huterm_bash_c1+=("$_huterm_bash_z")
    done
done
unset _huterm_bash_x _huterm_bash_y _huterm_bash_z

_huterm_bash_strip() {
    if [[ -n ${PS1+set} ]]; then
        PS1=${PS1//"$_huterm_bash_dmark"/}
        PS1=${PS1//"$_huterm_bash_amark"/}
        PS1=${PS1//"$_huterm_bash_bmark"/}
    fi
    if [[ -n ${PS0+set} ]]; then
        PS0=${PS0//"$_huterm_bash_cmark$_huterm_bash_ranmark"/}
        PS0=${PS0//"$_huterm_bash_cmark"/}
    fi
    return 0
}
_huterm_bash_disable() {
    _huterm_bash_disabled=1
    # During loading, the bootstrap then restores the user's own trap.
    unset _huterm_bash_trap
    _huterm_bash_running=0
    _huterm_bash_strip
    if [[ $_huterm_bash_ps0_unset == 1 && -n ${PS0+set} && -z $PS0 ]]; then
        unset PS0
    fi
    _huterm_bash_compose remove
    return 0
}
_huterm_bash_failed() {
    _huterm_bash_disable
    printf 'huterm: shell integration failed to load: %s\n' "$_huterm_bash_body_path" >&2
}
_huterm_bash_cwd() {
    local LC_ALL=C value=${HOSTNAME-}$PWD sequence
    case $value in
        *[[:cntrl:]]*) return 0 ;;
        *$'\302'*)
            for sequence in "${_huterm_bash_c1[@]}"; do
                case $value in *"$sequence"*) return 0 ;; esac
            done
            ;;
    esac
    printf '\e]7;kitty-shell-cwd://%s%s\a' "${HOSTNAME-}" "$PWD"
}
# The first prompt-command entry on Bash 3.2, and when promptvars is off. It
# restores $? and $_ for the entries after it.
_huterm_bash_capture() {
    _huterm_bash_status=$?
    _huterm_bash_captured=1
    _huterm_bash_armed=0
    return "$_huterm_bash_status"
}
_huterm_bash_keep() { return 0; }
_huterm_bash_restore() { return "$1"; }
_huterm_bash_prompt_array() {
    if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
        [[ -n ${PROMPT_COMMAND+set} && ${PROMPT_COMMAND@a} == *a* ]]; then
        return 0
    fi
    return 1
}
# Removes Huterm's entries from one prompt-command entry and any separators
# that only joined other text to them. Sets _huterm_bash_cleaned.
_huterm_bash_clean() {
    local rest=$1 line out= first=1
    _huterm_bash_cleaned=
    while :; do
        line=${rest%%$'\n'*}
        case $line in
            *_huterm_bash_*)
                line=${line//"$_huterm_bash_capture_entry"/}
                line=${line//"$_huterm_bash_prompt_entry"/}
                while :; do
                    case $line in
                        [[:space:]]*|';'*) line=${line#?} ;;
                        '&&'*|'||'*) line=${line#??} ;;
                        *) break ;;
                    esac
                done
                while :; do
                    case $line in
                        *[[:space:]]|*'|') line=${line%?} ;;
                        *'&&') line=${line%??} ;;
                        *) break ;;
                    esac
                done
                if [[ -n $line && $line != ';' ]]; then
                    if [[ $first == 0 ]]; then out+=$'\n'; fi
                    out+=$line
                    first=0
                fi
                ;;
            *)
                if [[ $first == 0 ]]; then out+=$'\n'; fi
                out+=$line
                first=0
                ;;
        esac
        if [[ $rest != *$'\n'* ]]; then
            break
        fi
        rest=${rest#*$'\n'}
    done
    _huterm_bash_cleaned=$out
    return 0
}
_huterm_bash_trimmed() {
    local value=$1
    value=${value#"${value%%[![:space:]]*}"}
    value=${value%"${value##*[![:space:]]}"}
    _huterm_bash_cleaned=$value
}
# Rebuilds PROMPT_COMMAND with Huterm's entries on lines or elements of their
# own: the status capture first (after bash-preexec's own first entry), when it
# is needed, and the decoration last (before bash-preexec's final entry).
# "remove" leaves only the other entries.
_huterm_bash_compose() {
    local mode=${1-} entry first last capture=0 count index
    local -a entries=()
    _huterm_bash_bp_epilogue=0
    if _huterm_bash_prompt_array; then
        for entry in "${PROMPT_COMMAND[@]}"; do
            case $entry in
                *_huterm_bash_*)
                    _huterm_bash_clean "$entry"
                    if [[ -n $_huterm_bash_cleaned ]]; then
                        entries+=("$_huterm_bash_cleaned")
                    fi
                    ;;
                *) entries+=("$entry") ;;
            esac
        done
    else
        if [[ $_huterm_bash_ps0 == 0 ]] || ! shopt -q promptvars; then
            capture=1
        fi
        if [[ -n ${PROMPT_COMMAND-} ]]; then
            _huterm_bash_clean "$PROMPT_COMMAND"
            entry=$_huterm_bash_cleaned
            while [[ -n $entry ]]; do
                entries+=("${entry%%$'\n'*}")
                if [[ $entry != *$'\n'* ]]; then
                    break
                fi
                entry=${entry#*$'\n'}
            done
        fi
    fi
    if [[ $mode != remove ]]; then
        count=${#entries[@]}
        if (( count > 0 )); then
            _huterm_bash_trimmed "${entries[count - 1]}"
            if [[ $_huterm_bash_cleaned == __bp_interactive_mode ]]; then
                _huterm_bash_bp_epilogue=1
                last=${entries[count - 1]}
                unset 'entries[count - 1]'
                entries+=("$_huterm_bash_prompt_entry" "$last")
            else
                entries+=("$_huterm_bash_prompt_entry")
            fi
        else
            entries+=("$_huterm_bash_prompt_entry")
        fi
        if [[ $capture == 1 ]]; then
            _huterm_bash_trimmed "${entries[0]}"
            case $_huterm_bash_cleaned in
                __bp_precmd_invoke_cmd|'__bp_precmd_invoke_cmd "$_"')
                    first=${entries[0]}
                    entries=("$first" "$_huterm_bash_capture_entry" "${entries[@]:1}")
                    ;;
                *) entries=("$_huterm_bash_capture_entry" "${entries[@]}") ;;
            esac
        fi
    fi
    if _huterm_bash_prompt_array; then
        if [[ ${#entries[@]} == 0 ]]; then
            PROMPT_COMMAND=()
        else
            PROMPT_COMMAND=("${entries[@]}")
        fi
    else
        entry=
        index=0
        for first in "${entries[@]}"; do
            if (( index > 0 )); then entry+=$'\n'; fi
            entry+=$first
            index=1
        done
        if [[ ${PROMPT_COMMAND-} != "$entry" ]]; then
            PROMPT_COMMAND=$entry
        fi
    fi
    return 0
}
# The final prompt-command entry. It runs after user entries have built the
# prompt, so the marks wrap the prompt Bash is about to show.
_huterm_bash_prompt() {
    local code=$? ran=0 number bump=0 expand=0 dpart= epilogue=$_huterm_bash_bp_epilogue
    if [[ $_huterm_bash_disabled != 0 ]]; then
        return 0
    fi
    if [[ $_huterm_bash_ps0 == 1 ]]; then
        if shopt -q promptvars; then
            expand=1
        fi
        # Without promptvars, a rise in the command number (\#) means that a
        # command ran. Bash 5.3 lowers \# in ${var@P} by one only once a prompt
        # has set PS1, PS2, and PS0 (parse.y, decode_prompt_string), so its
        # first read, before any prompt, is one higher than later reads.
        number=${_huterm_bash_number_escape@P}
        if [[ -z $_huterm_bash_number ]]; then
            if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 3) )) &&
                [[ -n ${PS1+set} && -n ${PS2+set} ]]; then
                number=$((number - 1))
            fi
        elif (( number > _huterm_bash_number )); then
            bump=1
        fi
        _huterm_bash_number=$number
        if [[ $_huterm_bash_flagged == 1 ]]; then
            ran=$_huterm_bash_ran
        else
            ran=$bump
        fi
        _huterm_bash_ran=0
    else
        ran=$_huterm_bash_running
    fi
    _huterm_bash_running=0
    if [[ $_huterm_bash_marked != 1 ]]; then
        ran=0
    fi
    if [[ $ran == 1 ]]; then
        if [[ $expand == 1 ]]; then
            dpart=$_huterm_bash_dmark
        else
            if [[ $_huterm_bash_captured == 1 ]]; then
                code=$_huterm_bash_status
            elif ! _huterm_bash_prompt_array; then
                code=
            fi
            # Array entries each start from the command's state, so $? is exact.
            printf '\e]133;D%s\a' "${code:+;$code}" || { _huterm_bash_failed; return 0; }
        fi
    fi
    _huterm_bash_captured=0
    _huterm_bash_strip
    _huterm_bash_marked=0
    case ${PS1-}${PS2-} in
        *$'\e]133;'*|*'\e]133;'*|*'\E]133;'*|*'\033]133;'*|*'\x1b]133;'*) ;;
        *) _huterm_bash_marked=1 ;;
    esac
    _huterm_bash_cwd || { _huterm_bash_failed; return 0; }
    # Feature: path. Delay until all startup files have completed.
    if [[ $_huterm_bash_features == *,path,* && -n ${HUTERM_BIN_DIR-} ]]; then
        case :$PATH: in
            *:"$HUTERM_BIN_DIR":*) ;;
            *) export PATH=$PATH:$HUTERM_BIN_DIR ;;
        esac
    fi
    if [[ $_huterm_bash_marked == 1 ]]; then
        PS1=$dpart$_huterm_bash_amark${PS1-}$_huterm_bash_bmark
        if [[ $_huterm_bash_ps0 == 1 ]]; then
            if [[ $expand == 1 ]]; then
                PS0=${PS0-}$_huterm_bash_cmark$_huterm_bash_ranmark
                _huterm_bash_flagged=1
            else
                PS0=${PS0-}$_huterm_bash_cmark
                _huterm_bash_flagged=0
            fi
        fi
    elif [[ $_huterm_bash_ps0 == 1 ]]; then
        # Keep PS0 set, which keeps Bash 5.3's \# reads consistent.
        PS0=${PS0-}
        _huterm_bash_flagged=0
    fi
    _huterm_bash_compose
    _huterm_bash_armed=1
    # bash-preexec installed itself earlier in this prompt-command run and armed
    # its next-command detection; running this entry after it disarmed that.
    if [[ $epilogue == 0 && $_huterm_bash_bp_epilogue == 1 ]] && declare -F __bp_interactive_mode >/dev/null; then
        __bp_interactive_mode force
    fi
}
# A DEBUG trap that runs the user's trap verbatim, at trap scope, for every
# command except Huterm's own, so preexec-style traps do not mistake Huterm's
# prompt entries for the next command. On Bash 3.2, which has no PS0, it also
# marks execution.
_huterm_bash_debug() {
    _huterm_bash_debug_status=$1
    _huterm_bash_debug_argument=$2
    _huterm_bash_chain=0
    local internal=0 frame
    case $BASH_COMMAND in *_huterm_bash_*) internal=1 ;; esac
    for frame in "${FUNCNAME[@]:1}"; do
        case $frame in _huterm_bash_*) internal=1; break ;; esac
    done
    if [[ $internal == 0 ]]; then
        if [[ -n $_huterm_bash_prior_debug ]]; then
            _huterm_bash_chain=1
        fi
        # bash-preexec's final prompt-command entry runs after Huterm's.
        if [[ $_huterm_bash_ps0 == 0 && $_huterm_bash_armed == 1 && $_huterm_bash_disabled == 0 &&
            ${BASH_SUBSHELL-0} == 0 && $BASH_COMMAND != __bp_interactive_mode ]]; then
            _huterm_bash_armed=0
            if [[ $_huterm_bash_marked == 1 ]]; then
                _huterm_bash_running=1
                printf '\e]133;C\a' || _huterm_bash_failed
            fi
        fi
    fi
    return 0
}
# The trap must be set at shell scope, because Bash 3.2 reverts DEBUG trap
# changes made inside an untraced function. The bootstrap has set the user's
# trap aside, and installs this one after its own statements.
if [[ -z ${_huterm_bash_saved_debug+set} ]]; then
    _huterm_bash_saved_debug=$(trap -p DEBUG)
    _huterm_bash_manual=1
fi
_huterm_bash_words=()
eval "_huterm_bash_words=($_huterm_bash_saved_debug)"
_huterm_bash_prior_debug=${_huterm_bash_words[2]-}
unset _huterm_bash_words
if [[ $_huterm_bash_ps0 == 0 || -n $_huterm_bash_prior_debug ]]; then
    _huterm_bash_trap='_huterm_bash_debug "$?" "$_"; '
    if [[ -n $_huterm_bash_prior_debug ]]; then
        _huterm_bash_trap+='if [[ $_huterm_bash_chain == 1 ]]; then _huterm_bash_restore "$_huterm_bash_debug_status" "$_huterm_bash_debug_argument"; '
        _huterm_bash_trap+=$_huterm_bash_prior_debug
        _huterm_bash_trap+=$'\nelse : "$_huterm_bash_debug_argument"; fi'
    else
        _huterm_bash_trap+=': "$_huterm_bash_debug_argument"'
    fi
fi
_huterm_bash_compose
if [[ -n ${_huterm_bash_manual-} ]]; then
    unset _huterm_bash_manual _huterm_bash_saved_debug
    if [[ -n ${_huterm_bash_trap-} ]]; then
        trap -- "$_huterm_bash_trap" DEBUG
        unset _huterm_bash_trap
    fi
fi
