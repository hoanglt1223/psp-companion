#!/usr/bin/env sh
# Installs companion-host for the current user from an unpacked release tarball:
#   ~/.local/bin/companion-host and a systemd user unit that starts it at login.
# Usage: ./install.sh [--no-start]   |   ./install.sh --uninstall
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
unit="companion-host.service"

have_systemd() {
    command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1
}

case "${1:-}" in
--uninstall)
    if have_systemd; then
        systemctl --user disable --now "$unit" 2>/dev/null || true
    fi
    rm -f "$bin_dir/companion-host" "$unit_dir/$unit"
    if have_systemd; then systemctl --user daemon-reload; fi
    echo "Removed companion-host. Config and pairings stay in ~/.config/psp-companion (delete it to forget them)."
    exit 0
    ;;
"" | --no-start) ;;
*)
    echo "usage: $0 [--no-start | --uninstall]" >&2
    exit 2
    ;;
esac

if [ ! -f "$here/companion-host" ]; then
    echo "companion-host not found next to install.sh; run it from the unpacked release folder" >&2
    exit 1
fi

mkdir -p "$bin_dir" "$unit_dir"
install -m 0755 "$here/companion-host" "$bin_dir/companion-host"
install -m 0644 "$here/$unit" "$unit_dir/$unit"
echo "Installed $bin_dir/companion-host and $unit_dir/$unit"

if [ "${1:-}" = "--no-start" ]; then
    echo "Not started. Start it with: systemctl --user enable --now $unit"
elif have_systemd; then
    systemctl --user daemon-reload
    systemctl --user enable --now "$unit"
    echo "Running. Logs: journalctl --user -u companion-host -f"
else
    echo "No systemd user session found; start it yourself: $bin_dir/companion-host --headless"
fi
echo "Settings and pairing (Pair new PSP shows the PIN): http://127.0.0.1:47863/settings"
