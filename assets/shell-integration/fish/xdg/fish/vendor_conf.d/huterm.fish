# Huterm Fish bootstrap. Vendor conf.d precedes the user's config.fish.
if test "$HUTERM_SHELL_INTEGRATION" != fish
    return
end
set -e HUTERM_SHELL_INTEGRATION
set -l original "$HUTERM_ORIG_XDG_DATA_DIRS"
set -l base (string sub -s 2 -- "$original")
if test -z "$base"
    set base /usr/local/share:/usr/share
end
set -l current (string join : -- $XDG_DATA_DIRS)
if test "$current" = "$HUTERM_XDG_ENTRY:$base"
    if string match -q '1*' -- "$original"
        set -gx XDG_DATA_DIRS (string sub -s 2 -- "$original")
    else
        set -e XDG_DATA_DIRS
    end
else if set -q XDG_DATA_DIRS
    set -l kept
    for component in (string split : -- "$current")
        if test "$component" != "$HUTERM_XDG_ENTRY"
            set -a kept "$component"
        end
    end
    set -gx XDG_DATA_DIRS (string join : -- $kept)
end
set -e HUTERM_ORIG_XDG_DATA_DIRS
set -e HUTERM_XDG_ENTRY
status is-interactive; or return
set -l body "$HUTERM_RESOURCES_DIR/shell-integration/fish/huterm.fish"
set -l fish_binary (status fish-path)
if "$fish_binary" --no-config --no-execute "$body" 2>/dev/null
    source "$body"; and return
end
functions -q _huterm_fish_disable; and _huterm_fish_disable
printf 'huterm: shell integration failed to load: %s\n' "$body" >&2
