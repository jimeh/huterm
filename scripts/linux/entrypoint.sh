#!/usr/bin/env bash
set -euo pipefail

mkdir -p /cache/cargo-registry /cache/cargo-git

# Keep host artifacts out of Linux builds. --delete removes obsolete source
# files but preserves excluded caches in this worktree/architecture's volume.
rsync -a --delete \
  --exclude '/.git' --exclude '/target' --exclude '/.native' \
  --exclude '/node_modules' --exclude '/.codegraph' --exclude '.DS_Store' \
  /source/ /workspace/

# Source is copied rather than edited through the host mount. Git metadata
# is omitted because a linked worktree's .git points outside that mount.
exec mise exec -- "$@"
