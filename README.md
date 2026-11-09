# Wayfinder

An accessible GTK4 file manager built in Rust, designed for tiled window managers and screen reader users.

Wayfinder puts accessibility first. Every element is navigable with a keyboard, announced by Orca, and designed for users who may never touch a mouse.

## Features

- **Full keyboard navigation** with arrow keys, Tab cycling, type-ahead file search, and wrapping menus
- **Screen reader support** via Orca with contextual announcements for every action
- **Sidebar** with places, bookmarks (Ctrl+D), mounted volumes, and editable layout
- **Context menu** with accessible roles, submenu navigation (Right/Left arrows), and dynamic actions
- **Custom actions** loaded from `.desktop` files -- extract archives, compress files, GPG encrypt/decrypt, and more
- **Nautilus scripts** support (`~/.local/share/nautilus/scripts/`)
- **FMA compatibility** (FileManager-Actions `.desktop` files from `/usr/share/file-manager/actions/`)
- **Drag and drop** between Wayfinder and other applications
- **Tab path completion** in the location bar with `~` expansion
- **Volume management** -- mount, unmount, and eject removable media from the sidebar
- **D-Bus FileManager1** service so other applications can open folders in Wayfinder
- **Copy/move with progress** dialog including time estimates and cancellation
- **Replace/Skip/Rename** conflict dialog when pasting over existing files
- **FUSE mount support** -- rclone, sshfs, and other FUSE mounts work without freezing. Async file monitoring, trash fallback to permanent delete, cross-device directory move
- **Wayfinder Sync** -- what you keep offline on an rclone drive stays in step with it both ways, like a native cloud app: your changes are sent up seconds after you save, changes made elsewhere come down within minutes, files kept offline open from the offline copy, and you're told what changed. A background service (`wayfinder-sync`), set up by a guide the first time you keep something offline
- **Session restore** -- all open windows and their directories are remembered on exit and restored on next launch
- **Font zoom** -- Ctrl+Plus/Minus/0 to zoom in, out, or reset (50%–300%, persisted)
- **Breadcrumb path bar** -- clickable path segments for quick navigation, F4 or header button to toggle
- **Recent files** -- sidebar entry showing recently opened files
- **Batch rename** -- find/replace across multiple selected files with live preview
- **Permission editor** -- interactive chmod in the Properties dialog with checkboxes and octal entry
- **List and grid views** switchable with Ctrl+1 / Ctrl+2
- **Bin integration** -- move to bin, restore, empty, with accessible confirmation dialogs

## Requirements

- GTK 4.14 or later
- Rust 1.70+
- `gvfs` and `udisks2` for volume management
- Optional: `bsdtar`, `zip`, `gpg`, `seahorse`, `zenity` for context menu actions
- Optional: `rclone` for direct rclone operations and Wayfinder Sync (which also needs a systemd user session)

## Building

```bash
cargo build --release --bin wayfinder --bin wayfinder-sync
```

The binaries are at `target/release/`. Copy both to `~/.local/bin/` or `/usr/bin/` — `wayfinder-sync` must sit beside `wayfinder`, where Wayfinder's setup guide looks for it.

To install the bundled context menu actions:

```bash
mkdir -p ~/.local/share/wayfinder/actions
cp data/actions/*.desktop ~/.local/share/wayfinder/actions/
```

Or for system-wide installation:

```bash
install -Dm755 target/release/wayfinder /usr/bin/wayfinder
install -Dm755 target/release/wayfinder-sync /usr/bin/wayfinder-sync
install -Dm644 data/actions/*.desktop -t /usr/share/wayfinder/actions/
```

## Usage

```bash
wayfinder                    # Restores previous session (or opens home directory)
wayfinder /path/to/folder    # Opens a specific directory
```

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| Enter | Open file or folder |
| Backspace | Go to parent directory |
| Alt+Left | Go back |
| Alt+Right | Go forward |
| Alt+Up | Go to parent directory |
| Ctrl+L | Focus location bar |
| Tab (in location bar) | Path completion |
| Ctrl+D | Bookmark current folder |
| Delete (on bookmark) | Remove bookmark |
| Ctrl+Up/Down (on bookmark) | Reorder bookmark |
| Ctrl+H | Toggle hidden files |
| Ctrl+1 / Ctrl+2 | Switch to grid / list view |
| F3 | Toggle sidebar |
| F4 | Toggle breadcrumb bar |
| Ctrl+F | Search files |
| Ctrl+A | Select all |
| Space | Toggle selection on focused file |
| Shift+Space | Range selection |
| Escape | Clear selection |
| Ctrl+C | Copy (all windows) |
| Ctrl+X | Cut (all windows) |
| Ctrl+V | Paste (all windows) |
| Ctrl+Shift+C | Copy (this window only) |
| Ctrl+Shift+X | Cut (this window only) |
| Ctrl+Shift+V | Paste (this window only) |
| F2 | Rename |
| Ctrl+Shift+F2 | Batch rename (multiple selected files) |
| Ctrl+Shift+N | New folder |
| Delete | Move to Bin |
| Shift+Delete | Delete permanently |
| Ctrl+Z | Undo trash (restore last trashed) |
| Ctrl+N | New window |
| Ctrl+` | Open terminal in current directory |
| Ctrl+I | Properties |
| Shift+F10 / Menu | Context menu |
| Ctrl+Plus / Ctrl+= | Zoom in |
| Ctrl+Minus | Zoom out |
| Ctrl+0 | Reset zoom |
| Ctrl+? | Keyboard shortcuts window |
| Tab | Cycle focus: file list → navigation buttons → location bar |
| Type any letters | Jump to matching file |

### Custom actions

Wayfinder loads context menu actions from `.desktop` files in these directories (first match wins):

1. `~/.local/share/wayfinder/actions/` -- user actions
2. `~/.local/share/file-manager/actions/` -- FMA user actions
3. `/usr/share/file-manager/actions/` -- FMA system actions (apps install here)
4. `/usr/share/wayfinder/actions/` -- system Wayfinder actions
5. `/usr/local/share/wayfinder/actions/` -- system Wayfinder actions (alternate prefix)
6. `~/.local/share/nautilus/scripts/` -- Nautilus scripts

Action file format:

```ini
[Desktop Entry]
Name=Extract Here
TryExec=bsdtar
Exec=sh -c 'cd "$(dirname "$1")" && bsdtar xf "$1"' _ %f
MimeType=application/zip;application/x-tar;
```

`TryExec` makes the action invisible if the command is not installed. `MimeType` restricts which files the action appears for (omit for all files). Exec supports `%f` (single file), `%F` (multiple files), `%u` (single URI), `%U` (multiple URIs).

### D-Bus integration

Wayfinder registers as `org.freedesktop.FileManager1`. Other applications can open folders:

```bash
gdbus call --session \
  -d org.freedesktop.FileManager1 \
  -o /org/freedesktop/FileManager1 \
  -m org.freedesktop.FileManager1.ShowFolders \
  "['file:///home']" ""
```

### Sidebar configuration

Right-click or press Menu on the sidebar to access "Edit Sidebar". Toggle places on or off, and reorder with Ctrl+Up/Down. Configuration is saved in `~/.config/wayfinder/sidebar`.

Bookmarks are stored in `~/.config/gtk-3.0/bookmarks`, compatible with Nautilus and other GTK file managers.

## Architecture

Wayfinder is a pure Rust GTK4 application using the gtk-rs bindings. No libadwaita dependency.

```
src/
  app/           Application subclass, D-Bus startup
  window/        Main window, keyboard navigation, actions
  context_menu/  Accessible popover menu with submenu support
  sidebar/       Places, bookmarks, volumes, edit dialog
  file_model/    Directory loading, sorting, filtering, file monitor
  file_object/   GObject data model for file entries
  file_ops/      Copy, move, trash, delete with progress and conflict dialogs
  views/         List view (ColumnView) and grid view with drag sources
  actions/       Custom action loading and execution
  dbus/          org.freedesktop.FileManager1 service
  navigation/    Back/forward/up history stack
  search/        File name search/filter
  shortcuts/     Keyboard shortcut registration
  properties/    File properties and permission editor
  clipboard/     Global and window-local clipboard
  state.rs       Persistent state (window size, sort, zoom, sidebar, session restore)
  portal/        XDG Desktop Portal file chooser backend (separate binary)
```

## Licence

GPL-3.0-or-later
