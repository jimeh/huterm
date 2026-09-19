#!/bin/sh
# Stage and run host-built Huterm commands inside a disposable macOS guest.
set -eu

share="/Volumes/My Shared Files/huterm"
workspace="$HOME/huterm"

case "${1:-}" in
  stage)
    # Copy instead of running from the share: virtiofs returns ELOOP for
    # symlink extended attributes, which breaks ditto and cp on frameworks.
    # rsync -a omits extended attributes. Only host-built runtime outputs
    # are staged; the guest never compiles.
    rsync -a --delete --prune-empty-dirs \
      --include=/target/ --include=/target/debug/ --include=/target/debug/examples/ \
      --include='/target/debug/examples/*_smoke' --include='/target/debug/*-witness' \
      --include=/target/debug/huterm --include=/target/terminfo/ --include='/target/terminfo/**' \
      --exclude='/target/**' \
      --include=/.native/ --include=/.native/sparkle/ --include=/.native/sparkle/distribution/ \
      --include='/.native/sparkle/distribution/**' --exclude='/.native/**' \
      --exclude=/.git --exclude=/node_modules --exclude=/dist --exclude=/.codegraph \
      "$share/" "$workspace/"
    ;;
  exec)
    shift
    [ "$#" -gt 0 ] || { echo "guest.sh exec requires a command" >&2; exit 2; }
    cd "$workspace"
    export MISE_AUTO_INSTALL=false MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS="$workspace"
    exec "$@"
    ;;
  *)
    echo "usage: guest.sh stage | exec <command> [argument ...]" >&2
    exit 2
    ;;
esac
