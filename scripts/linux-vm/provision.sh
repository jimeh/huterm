#!/bin/sh
# Provision Huterm's interactive Linux image inside a Cirrus Labs Ubuntu guest.
# Runs through `tart exec` with the repository shared read-only.
set -eu

export DEBIAN_FRONTEND=noninteractive
sudo -E apt-get update -qq

# GNOME supplies both the Wayland and Xorg sessions; Mesa supplies the software
# Vulkan device GPUI needs without a virtualized GPU.
# network-manager is required, not optional: ubuntu-desktop ships
# /usr/lib/netplan/00-network-manager-all.yaml, so netplan renders every
# interface through NetworkManager and the guest has no network without it.
sudo -E apt-get install -y -qq --no-install-recommends \
  ubuntu-desktop-minimal network-manager mesa-vulkan-drivers fonts-dejavu-core \
  x11-utils xdotool

# GNOME aborts its Wayland session with "No GSettings schemas are installed"
# unless the compiled schema cache is rebuilt after this install; GDM then
# silently falls back to Xorg.
sudo glib-compile-schemas /usr/share/glib-2.0/schemas/

# Log the desktop session in automatically. GDM picks the session from the
# user's AccountsService record, and WaylandEnable gates Wayland entirely;
# guest.sh rewrites both when a run selects the other session.
user=$(id -un)
printf '[daemon]\nAutomaticLoginEnable=true\nAutomaticLogin=%s\nWaylandEnable=true\n' "$user" \
  | sudo tee /etc/gdm3/custom.conf >/dev/null
sudo mkdir -p /var/lib/AccountsService/users
printf '[User]\nSession=ubuntu-wayland\nXSession=ubuntu-xorg\nSystemAccount=false\n' \
  | sudo tee "/var/lib/AccountsService/users/$user" >/dev/null
echo /usr/sbin/gdm3 | sudo tee /etc/X11/default-display-manager >/dev/null
sudo systemctl enable gdm3 >/dev/null 2>&1

# Idle blanking and the lock screen stop GPUI frames and swallow input during
# long manual sessions. Unattended upgrades would take apt locks unprompted.
sudo mkdir -p /etc/dconf/db/local.d /etc/dconf/profile
sudo sh -c 'printf "user-db:user\nsystem-db:local\n" > /etc/dconf/profile/user'
sudo sh -c 'printf "[org/gnome/desktop/session]\nidle-delay=uint32 0\n\n[org/gnome/desktop/screensaver]\nlock-enabled=false\nidle-activation-enabled=false\n" > /etc/dconf/db/local.d/00-huterm'
sudo dconf update
sudo systemctl disable --now unattended-upgrades >/dev/null 2>&1 || true

# This guest's NAT interface never satisfies systemd-networkd-wait-online, so
# the unit times out after two minutes and delays graphical.target on every
# boot. Bound the wait with a drop-in file: a mask symlink does not survive
# cloning, while regular files under /etc do.
sudo mkdir -p /etc/systemd/system/systemd-networkd-wait-online.service.d
printf '[Service]\nExecStart=\nExecStart=/usr/lib/systemd/systemd-networkd-wait-online --any --timeout=5\n' \
  | sudo tee /etc/systemd/system/systemd-networkd-wait-online.service.d/override.conf >/dev/null

# Mount the host share at boot so `tart exec` can read guest.sh from it.
sudo sh -c 'printf "com.apple.virtio-fs.automount /mnt/shared virtiofs defaults,nofail 0 0\n" >> /etc/fstab'
sudo mkdir -p /mnt/shared

sudo -E apt-get clean

# The image is published as soon as this exits, and the VM is stopped without
# waiting for writeback, so late provisioning writes would otherwise be lost.
sudo sync
