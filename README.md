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
- **List and grid views** switchable with Ctrl+1 / Ctrl+2
- **Bin integration** -- move to bin, restore, empty, with accessible confirmation dialogs

## Requirements

- GTK 4.14 or later
- Rust 1.70+
- `gvfs` and `udisks2` for volume management
- Optional: `bsdtar`, `zip`, `gpg`, `seahorse`, `zenity` for context menu actions

## Building

```bash
cargo build --release --bin wayfinder
```

The binary is at `target/release/wayfinder`. Copy it to `~/.local/bin/` or `/usr/bin/`.

To install the bundled context menu actions:

```bash
mkdir -p ~/.local/share/wayfinder/actions
cp data/actions/*.desktop ~/.local/share/wayfinder/actions/
```

Or for system-wide installation:

```bash
install -Dm755 target/release/wayfinder /usr/bin/wayfinder
install -Dm644 data/actions/*.desktop -t /usr/share/wayfinder/actions/
```

## Usage

```bash
wayfinder                    # Opens home directory (or last visited)
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
| Ctrl+1 / Ctrl+2 | Switch to list / grid view |
| Ctrl+Shift+S | Toggle sidebar |
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
| Ctrl+Shift+N | New folder |
| Delete | Move to Bin |
| Shift+Delete | Delete permanently |
| Ctrl+Z | Undo trash (restore last trashed) |
| Ctrl+N | New window |
| Ctrl+` | Open terminal in current directory |
| Ctrl+I | Properties |
| Shift+F10 / Menu | Context menu |
| Ctrl+? | Keyboard shortcuts window |
| Type any letters | Jump to matching file |

### Custom actions

Wayfinder loads context menu actions from `.desktop` files in these directories (first match wins):

1. `~/.local/share/wayfinder/actions/` -- user actions
2. `~/.local/share/file-manager/actions/` -- FMA user actions
3. `/usr/share/file-manager/actions/` -- FMA system actions (apps install here)
4. `/usr/share/wayfinder/actions/` -- system Wayfinder actions
5. `~/.local/share/nautilus/scripts/` -- Nautilus scripts

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
  file_ops/      Copy, move, trash, delete with progress dialogs
  views/         List view (ColumnView) and grid view with drag sources
  actions/       Custom action loading and execution
  dbus/          org.freedesktop.FileManager1 service
  navigation/    Back/forward/up history stack
  search/        File name search/filter
  shortcuts/     Keyboard shortcut registration
  properties/    File properties dialog
  state/         Persistent state (window size, sort, sidebar config)
```

## Licence

GPL-3.0-or-later
