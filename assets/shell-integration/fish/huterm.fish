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
function _huterm_fish_prompt --on-event fish_prompt
    # Feature: path. This event follows config.fish.
    if contains -- path $_huterm_fish_features; and test -n "$HUTERM_BIN_DIR"
        if not contains -- "$HUTERM_BIN_DIR" $PATH
            set -gx PATH $PATH "$HUTERM_BIN_DIR"; or _huterm_fish_failed
        end
    end
end
