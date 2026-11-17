# Huterm Fish integration. Fish 4.9.3 supplies A, B, C, D, and OSC 7 itself.
status is-interactive; or return
set -q _huterm_fish_loaded; and return
set -g _huterm_fish_loaded 1
set -g _huterm_fish_features (string split , -- "$HUTERM_SHELL_FEATURES")
function _huterm_fish_disable
    functions -e _huterm_fish_prompt
end
function _huterm_fish_failed
    _huterm_fish_disable
    printf 'huterm: shell integration failed to load: %s\n' "$HUTERM_RESOURCES_DIR/shell-integration/fish/huterm.fish" >&2
end
# Features: ssh_env, ssh_terminfo, sudo. Runs `huterm ssh` or `huterm sudo`
# when the huterm command is available, and the program itself otherwise.
function _huterm_fish_run
    set -l tool $argv[1]
    set -e argv[1]
    if test -n "$HUTERM_BIN_DIR"; and test -x "$HUTERM_BIN_DIR/huterm"
        set -l flags
        if test "$tool" = ssh
            set -l features (string split , -- "$HUTERM_SHELL_FEATURES")
            contains -- ssh_env $features; and set -a flags --forward-env
            contains -- ssh_terminfo $features; and set -a flags --terminfo
        end
        "$HUTERM_BIN_DIR/huterm" $tool $flags $argv
    else
        command $tool $argv
    end
end
# Defines the wrappers once, after config.fish, unless a function of that
# name exists, autoloaded or defined by an alias included.
function _huterm_fish_wrap
    set -g _huterm_fish_wrapped 1
    if contains -- ssh_env $_huterm_fish_features; or contains -- ssh_terminfo $_huterm_fish_features
        if not functions -q ssh
            function ssh
                _huterm_fish_run ssh $argv
            end
        end
    end
    if contains -- sudo $_huterm_fish_features; and not functions -q sudo
        function sudo
            _huterm_fish_run sudo $argv
        end
    end
end
function _huterm_fish_prompt --on-event fish_prompt
    # Feature: path. This event follows config.fish.
    if contains -- path $_huterm_fish_features; and test -n "$HUTERM_BIN_DIR"
        if not contains -- "$HUTERM_BIN_DIR" $PATH
            set -gx PATH $PATH "$HUTERM_BIN_DIR"; or _huterm_fish_failed
        end
    end
    set -q _huterm_fish_wrapped; or _huterm_fish_wrap
end
