#!/usr/bin/env bash
set -euo pipefail

sync_workspace() {
  local source_root="$1"
  local workspace_root="$2"
  local clean_dist="${3:-0}"

  # Excluded directories survive rsync --delete, so package runs must remove
  # retained output before syncing source that deliberately excludes host dist.
  if [[ "$clean_dist" == 1 ]]; then
    rm -rf -- "$workspace_root/dist"
  fi
  rsync -a --delete \
    --exclude '/.git' --exclude '/target' --exclude '/.native' \
    --exclude '/node_modules' --exclude '/.codegraph' --exclude '/dist' \
    --exclude '.DS_Store' "$source_root/" "$workspace_root/"
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  return 0
fi

mkdir -p /cache/cargo-registry /cache/cargo-git

# Keep host artifacts out of Linux builds while preserving excluded caches in
# this worktree/architecture's workspace volume.
sync_workspace /source /workspace "${HUTERM_LINUX_CLEAN_DIST:-0}"

# Source is copied rather than edited through the host mount. Git metadata
# is omitted because a linked worktree's .git points outside that mount.
exec mise exec -- "$@"
