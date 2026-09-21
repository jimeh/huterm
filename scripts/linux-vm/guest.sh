#!/bin/sh
# Drive the desktop session of a Huterm Linux VM from `tart exec`, which runs
# outside that session and inherits none of its display environment.
set -eu

share_root=/mnt/shared
workspace="$share_root/huterm"
runtime_dir="/run/user/$(id -u)"

session_id() {
  loginctl list-sessions --no-legend 2>/dev/null | awk -v user="$(id -un)" '$3 == user { print $1; exit }'
}

session_type() {
  id=$(session_id)
  if [ -n "$id" ]; then
    loginctl show-session "$id" -p Type --value 2>/dev/null || echo none
  else
    echo none
  fi
}

session_display() {
  XDG_RUNTIME_DIR="$runtime_dir" DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus" \
    systemctl --user show-environment 2>/dev/null | grep -E '^DISPLAY=' || true
}

mount_share() {
  mountpoint -q "$share_root" && return 0
  sudo mkdir -p "$share_root"
  sudo mount -t virtiofs com.apple.virtio-fs.automount "$share_root"
}

# A session reports its type before publishing DISPLAY; GPUI's X11 backend needs
# both, on Xorg and through XWayland.
wait_ready() {
  want=$1
  deadline=$(( $(date +%s) + 180 ))
  while :; do
    if [ "$(session_type)" = "$want" ] && [ -n "$(session_display)" ]; then
      return 0
    fi
    [ "$(date +%s)" -lt "$deadline" ] || {
      echo "desktop session did not reach $want with a display: type=$(session_type)" >&2
      exit 1
    }
    sleep 2
  done
}

case "${1:-}" in
  session-type)
    # Autologin publishes a session seconds after the guest agent answers;
    # reporting "none" too early would request a needless GDM restart.
    deadline=$(( $(date +%s) + 90 ))
    while [ "$(session_type)" = none ] && [ "$(date +%s)" -lt "$deadline" ]; do
      sleep 2
    done
    session_type
    ;;
  set-session)
    want=${2:-}
    case "$want" in
      wayland) enable=true; desktop=ubuntu-wayland ;;
      x11) enable=false; desktop=ubuntu-xorg ;;
      *) echo "set-session requires wayland or x11" >&2; exit 2 ;;
    esac
    user=$(id -un)
    # GDM reads the session from AccountsService and gates Wayland separately.
    printf '[daemon]\nAutomaticLoginEnable=true\nAutomaticLogin=%s\nWaylandEnable=%s\n' "$user" "$enable" \
      | sudo tee /etc/gdm3/custom.conf >/dev/null
    printf '[User]\nSession=%s\nXSession=ubuntu-xorg\nSystemAccount=false\n' "$desktop" \
      | sudo tee "/var/lib/AccountsService/users/$user" >/dev/null
    sudo systemctl restart gdm3
    wait_ready "$want"
    ;;
  wait-ready)
    wait_ready "${2:-wayland}"
    ;;
  run)
    shift
    [ "$#" -gt 0 ] || { echo "guest.sh run requires a command" >&2; exit 2; }
    mount_share
    export XDG_RUNTIME_DIR="$runtime_dir"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus"
    # XAUTHORITY names a per-boot mutter file under XWayland, so read it now.
    eval "$(systemctl --user show-environment \
      | grep -E '^(DISPLAY|XAUTHORITY|WAYLAND_DISPLAY|XDG_SESSION_TYPE|XDG_SESSION_DESKTOP|XDG_CURRENT_DESKTOP|GNOME_SHELL_SESSION_MODE)=' \
      | sed 's/^/export /')"
    export HUTERM_VM_WORKSPACE="$workspace"
    cd "$HOME"
    exec "$@"
    ;;
  *)
    echo "usage: guest.sh session-type | set-session <wayland|x11> | wait-ready <session> | run <command ...>" >&2
    exit 2
    ;;
esac
