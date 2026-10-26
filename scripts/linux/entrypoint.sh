#!/usr/bin/env bash
set -euo pipefail

# Arguments after the first three go to rsync.
sync_workspace() {
  local source_root="$1"
  local workspace_root="$2"
  local clean_dist="${3:-0}"
  shift "$(($# < 3 ? $# : 3))"

  # Excluded directories survive rsync --delete, so package runs must remove
  # retained output before syncing source that deliberately excludes host dist.
  if [[ "$clean_dist" == 1 ]]; then
    rm -rf -- "$workspace_root/dist"
  fi
  rsync -a --delete "$@" \
    --exclude '/.git' --exclude '/target' --exclude '/.native' \
    --exclude '/node_modules' --exclude '/.codegraph' --exclude '/dist' \
    --exclude '.DS_Store' "$source_root/" "$workspace_root/"
}

# Older images ran as root, and a worktree that switches to a branch still
# building one writes root-owned files into the same volumes, so every run
# hands back whatever the huterm user does not own.
adopt_volume() {
  find "$1" ! -user huterm -exec chown -h huterm:huterm {} +
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  return 0
fi

# Keep host artifacts out of Linux builds while preserving excluded caches in
# this worktree/architecture's workspace volume. Root copies the source, which
# the huterm user may not be able to read, and the command runs as huterm.
if [[ "$(id -u)" == 0 ]]; then
  sync_workspace /source /workspace "${HUTERM_LINUX_CLEAN_DIST:-0}" \
    --chown=huterm:huterm
  mkdir -p /cache/cargo-registry /cache/cargo-git
  adopt_volume /workspace
  adopt_volume /cache
  # Source is copied rather than edited through the host mount. Git metadata
  # is omitted because a linked worktree's .git points outside that mount.
  exec setpriv --reuid=huterm --regid=huterm --init-groups \
    env HOME=/home/huterm USER=huterm LOGNAME=huterm mise exec -- "$@"
fi

mkdir -p /cache/cargo-registry /cache/cargo-git
sync_workspace /source /workspace "${HUTERM_LINUX_CLEAN_DIST:-0}"
exec mise exec -- "$@"
