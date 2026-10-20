#!/bin/sh
# Provision Huterm's macOS smoke image inside a Cirrus Labs base guest.
# Runs through `tart exec` with the repository shared read-only.
set -eu

share="/Volumes/My Shared Files/huterm"

# Match scripts/linux/Dockerfile's Mise release; the image's Homebrew copy is older.
mise_version=2026.10.12
mise_sha=6dc12b6087d4887cf807e9a9f3c2aa0ae0d90a6f465c6bd9c5d4a07847f88ab3
archive=$(mktemp -d)
curl -fsSL "https://github.com/jdx/mise/releases/download/v${mise_version}/mise-v${mise_version}-macos-arm64.tar.gz" \
  -o "$archive/mise.tar.gz"
echo "$mise_sha  $archive/mise.tar.gz" | shasum -a 256 -c -
tar -xzf "$archive/mise.tar.gz" -C "$archive"
sudo install -m 0755 "$archive/mise/bin/mise" /usr/local/bin/mise
rm -rf "$archive"

export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1
if brew list --formula mise >/dev/null 2>&1; then
  brew uninstall --ignore-dependencies mise
fi
brew install tmux

# Install Bun from the lockfile; smoke workspaces reuse this global install.
tools="$HOME/huterm-tools"
mkdir -p "$tools"
cp "$share/mise.toml" "$share/mise.lock" "$tools/"
cd "$tools"
/usr/local/bin/mise trust --quiet "$tools/mise.toml"
MISE_YES=1 /usr/local/bin/mise install bun
/usr/local/bin/mise exec bun -- bun --version

# The image is published as soon as this exits, and the VM is stopped without
# waiting for writeback, so late provisioning writes would otherwise be lost.
sudo sync
