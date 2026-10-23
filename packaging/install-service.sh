#!/bin/sh
# Installs LANrurugi as a system service for this platform, using this package's location as the
# program prefix and <data-dir> for the library, thumbnails, temp files, logs and the env file.
#
# Usage: ./install-service.sh <data-dir> [--print]
#   Linux   → /etc/systemd/system/lanrurugi.service   (run as root)
#   macOS   → ~/Library/LaunchAgents/org.lanrurugi.server.plist   (run as your user)
#   FreeBSD → /usr/local/etc/rc.d/lanrurugi   (run as root)
# --print writes the filled definition to stdout instead of installing it.
set -eu

data="${1:?usage: install-service.sh <data-dir> [--print]}"
mode="${2:-install}"
prefix="$(cd "$(dirname "$0")" && pwd)"
case "$data" in /*) ;; *) data="$(pwd)/$data" ;; esac
user="${SUDO_USER:-$(id -un)}"

fill() {
    sed -e "s|@PREFIX@|$prefix|g" -e "s|@DATA@|$data|g" -e "s|@USER@|$user|g" "$1"
}

case "$(uname -s)" in
    Linux) template=lanrurugi.service target=/etc/systemd/system/lanrurugi.service ;;
    Darwin) template=org.lanrurugi.server.plist target="$HOME/Library/LaunchAgents/org.lanrurugi.server.plist" ;;
    FreeBSD) template=lanrurugi.rc target=/usr/local/etc/rc.d/lanrurugi ;;
    *) echo "install-service: no service definition for $(uname -s)" >&2; exit 1 ;;
esac

if [ "$mode" = "--print" ]; then
    fill "$prefix/service/$template"
    exit 0
fi

mkdir -p "$data/content" "$data/thumb" "$data/temp" "$data/log" "$data/database"
if [ ! -f "$data/lanrurugi.env" ]; then
    fill "$prefix/service/lanrurugi.env.example" > "$data/lanrurugi.env"
    echo "install-service: wrote $data/lanrurugi.env — check LANRURUGI_LIBRARY_PATH and LANRURUGI_REDIS_URL"
fi
if [ -n "${SUDO_USER:-}" ]; then
    chown -R "$SUDO_USER" "$data"
fi

mkdir -p "$(dirname "$target")"
fill "$prefix/service/$template" > "$target"
case "$(uname -s)" in
    Linux)
        systemctl daemon-reload
        echo "install-service: installed $target — start with: systemctl enable --now lanrurugi" ;;
    Darwin)
        echo "install-service: installed $target — start with: launchctl load -w $target" ;;
    FreeBSD)
        chmod 555 "$target"
        echo "install-service: installed $target — start with: sysrc lanrurugi_enable=YES && service lanrurugi start" ;;
esac
