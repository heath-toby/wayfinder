#!/usr/bin/env bash
# Make Wayfinder's file chooser the Open and Save dialog for other apps
# (those that ask xdg-desktop-portal for one), or take it away again.
#
#   ./install-portal.sh               for you only: ~/.local/bin and your config
#   ./install-portal.sh --system      the program in /usr/bin, for everyone
#   ./install-portal.sh --uninstall   undo (add --system if installed that way)
#
# One step always needs root, because xdg-desktop-portal only looks for
# backends in /usr/share/xdg-desktop-portal/portals; it's skipped when the
# file there is already right. Which dialog your desktop uses is set in your
# own ~/.config/xdg-desktop-portal, so other users keep theirs.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
mode=user
action=install
for arg in "$@"; do
    case $arg in
        --system) mode=system ;;
        --uninstall) action=uninstall ;;
        -h | --help)
            sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

name=org.freedesktop.impl.portal.desktop.wayfinder
portal_file=/usr/share/xdg-desktop-portal/portals/wayfinder.portal
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
data_home=${XDG_DATA_HOME:-$HOME/.local/share}
if [ "$mode" = system ]; then
    bin=/usr/bin/wayfinder-portal
    service=/usr/share/dbus-1/services/$name.service
else
    bin=$HOME/.local/bin/wayfinder-portal
    service=$data_home/dbus-1/services/$name.service
fi

# Root only for what needs it: sudo in a terminal, else the desktop's
# password prompt.
as_root() {
    if [ "$(id -u)" = 0 ]; then
        "$@"
    elif [ -t 0 ] || ! command -v pkexec >/dev/null; then
        sudo "$@"
    else
        pkexec "$@"
    fi
}
for_mode() {
    if [ "$mode" = system ]; then as_root "$@"; else "$@"; fi
}

# The portals.conf that applies to this desktop: the first name in
# XDG_CURRENT_DESKTOP ("niri:GNOME" → niri-portals.conf).
desktop=${XDG_CURRENT_DESKTOP%%:*}
desktop=${desktop,,}
conf_name=${desktop:+$desktop-}portals.conf
conf=$config_home/xdg-desktop-portal/$conf_name
line_key=org.freedesktop.impl.portal.FileChooser

# What the desktop's settings say without ours: the system's file for this
# desktop, or the general one, or "anything".
system_conf() {
    local dir
    for dir in /etc/xdg/xdg-desktop-portal /usr/share/xdg-desktop-portal; do
        if [ -f "$dir/$conf_name" ]; then
            cat "$dir/$conf_name"
            return
        fi
    done
    for dir in /etc/xdg/xdg-desktop-portal /usr/share/xdg-desktop-portal; do
        if [ -f "$dir/portals.conf" ]; then
            cat "$dir/portals.conf"
            return
        fi
    done
    printf '[preferred]\ndefault=*\n'
}

# $1 with any FileChooser line taken out.
without_line() {
    grep -v "^[[:space:]]*$line_key[[:space:]]*=" <<<"$1" || true
}

restart_portal() {
    pkill -x wayfinder-porta 2>/dev/null || true
    if systemctl --user is-active --quiet xdg-desktop-portal.service 2>/dev/null; then
        systemctl --user restart xdg-desktop-portal.service
        echo "Restarted xdg-desktop-portal."
    else
        echo "xdg-desktop-portal isn't running; it will pick this up when it starts."
    fi
}

if [ "$action" = uninstall ]; then
    if [ -f "$conf" ]; then
        left=$(without_line "$(cat "$conf")")
        # Nothing of the user's own left: remove the file, so the system's
        # applies as before.
        if [ "$left" = "$(without_line "$(system_conf)")" ]; then
            rm -f "$conf"
            echo "Removed $conf"
        else
            printf '%s\n' "$left" >"$conf"
            echo "Took the file chooser line out of $conf"
        fi
    fi
    for_mode rm -f "$service" "$bin"
    if [ -f "$portal_file" ]; then
        echo "Removing $portal_file (needs root)"
        as_root rm -f "$portal_file"
    fi
    restart_portal
    echo "Done: apps get their usual file dialog again."
    exit 0
fi

# Build now, so what's installed is this source, never an older build.
echo "Building wayfinder-portal..."
(cd "$here" && cargo build --release --bin wayfinder-portal)

pkill -x wayfinder-porta 2>/dev/null || true
for_mode install -Dm755 "$here/target/release/wayfinder-portal" "$bin"
echo "Installed $bin"

for_mode install -d "$(dirname "$service")"
printf '[D-BUS Service]\nName=%s\nExec=%s\n' "$name" "$bin" | for_mode tee "$service" >/dev/null
echo "Installed $service"

if cmp -s "$here/data/wayfinder.portal" "$portal_file"; then
    echo "$portal_file is already in place."
else
    echo "Installing $portal_file (needs root)"
    as_root install -Dm644 "$here/data/wayfinder.portal" "$portal_file"
fi

# Your settings for this desktop: what they were (or the system's), with
# file dialogs going to Wayfinder first, then to whatever served them
# before — so if Wayfinder is ever removed, apps still get a dialog.
if [ -f "$conf" ]; then base=$(cat "$conf"); else base=$(system_conf); fi
base=$(without_line "$base")
fallback=$(sed -n 's/^[[:space:]]*default[[:space:]]*=[[:space:]]*//p' <<<"$base" | head -n1)
value="wayfinder;${fallback:-*}"
case $value in *';') ;; *) value="$value;" ;; esac
new=$(awk -v line="$line_key=$value" '
    { print }
    /^\[preferred\][[:space:]]*$/ && !done { print line; done = 1 }
    END { if (!done) { print "[preferred]"; print line } }
' <<<"$base")
mkdir -p "$(dirname "$conf")"
printf '%s\n' "$new" >"$conf"
echo "File dialogs now go to Wayfinder first ($conf: $line_key=$value)"

restart_portal
echo "Done. Apps that ask for a file dialog now get Wayfinder's."
echo "Some apps need a setting first: see \"File chooser for other apps\" in README.md."
