# Huterm Fish bootstrap. Vendor conf.d precedes the user's config.fish.
if test "$HUTERM_SHELL_INTEGRATION" != fish
    return
end
set -e HUTERM_SHELL_INTEGRATION
if string match -q '1*' -- "$HUTERM_ORIG_XDG_DATA_DIRS"
    set -gx XDG_DATA_DIRS (string sub -s 2 -- "$HUTERM_ORIG_XDG_DATA_DIRS")
else
    set -e XDG_DATA_DIRS
end
set -e HUTERM_ORIG_XDG_DATA_DIRS
set -e HUTERM_XDG_ENTRY
status is-interactive; or return
set -l body "$HUTERM_RESOURCES_DIR/shell-integration/fish/huterm.fish"
if fish --no-config --no-execute "$body" 2>/dev/null
    source "$body"; and return
end
functions -q _huterm_fish_disable; and _huterm_fish_disable
printf 'huterm: shell integration failed to load: %s\n' "$body" >&2
