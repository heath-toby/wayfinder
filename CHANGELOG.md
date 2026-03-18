# Changelog

## 2.2.1

### Fixed

- Restoring a file from the Bin now refreshes the trash listing immediately, with focus moving to the nearest remaining item.
- Trash failures (e.g. permission denied) are now announced via screen reader before any focus changes, so the error is heard first.

### Code quality

- Resolved all clippy warnings (unused imports, boolean simplification, const thread_local, identical branches).
- Updated dependencies to latest compatible patch versions.

---

## 2.2.0

### Context menu

- **Copy Path** -- copies the full file path to the system clipboard for pasting into terminals, text editors, etc. Announces "Copied path: /full/path".
- **Copy Name** -- copies just the filename to the system clipboard. Announces "Copied name: filename".

### Undo

- **Undo Trash (Ctrl+Z)** -- restores the most recently trashed file(s). Searches the Bin by original path, restores to the original location, and reloads the directory. Announces what was restored.

### Terminal

- **Open Terminal Here (Ctrl+`)** -- opens a terminal emulator in the current directory. Auto-detects foot, alacritty, gnome-terminal, or konsole.

### Accessibility

- **Sort order announcements** -- clicking a column header now announces "Sorted by Name, ascending" (or descending) via the screen reader.

---

## 2.1.0

### Clipboard

- **Cross-window copy/paste** -- Ctrl+C/X/V now uses a global clipboard shared across all Wayfinder windows. Copy in one window, paste in another.
- **Window-local clipboard** -- Ctrl+Shift+C/X/V for copy/cut/paste scoped to the current window only. Announcements say "(this window)" to distinguish.

### Keyboard shortcuts

- **Backspace** goes to parent directory (same as Alt+Up).
- **Ctrl+?** (Ctrl+Shift+/) opens a Keyboard Shortcuts window listing every shortcut organised by category: Navigation, File Operations, View, and General. Each entry is read by Orca as "Description: Key".
- **Ctrl+Shift+R** now opens File System (previously Ctrl+Shift+C, which is now window-local copy).

### Sidebar

- **Bookmark reordering** -- Ctrl+Up/Down reorders bookmarks directly in the sidebar. Announces "Moved above/below {name}" and "Already at top/bottom" at boundaries. Changes persist to the bookmarks file.

### Fixed

- Backspace shortcut was listed in documentation but never registered.

---

## 2.0.0

A major feature release focused on extensibility, integration, and accessibility.

### Sidebar

- **Bookmarks** -- Ctrl+D to bookmark the current folder. Delete key to remove. Compatible with `~/.config/gtk-3.0/bookmarks` (Nautilus/GTK format).
- **Edit Sidebar** -- right-click or Menu key to open the editor. Toggle places on/off, reorder with Ctrl+Up/Down and Ctrl+Shift+Home/End. Full screen reader announcements ("Moved above Documents", "Already at top").
- **Volume management** -- mounted and unmounted volumes appear in the sidebar with eject buttons. Volumes auto-update via GIO VolumeMonitor signals. Click to mount, eject button or right-click to unmount.

### Custom actions

- **Actions system** -- context menu actions loaded from `.desktop` files in `~/.local/share/wayfinder/actions/`, `/usr/share/file-manager/actions/` (FMA standard), and `/usr/share/wayfinder/actions/`.
- **TryExec** -- actions are hidden if their command is not installed, so bundled defaults work on any system.
- **Bundled actions** -- Extract Here (file-roller/ark/bsdtar), Compress (with format picker: zip, tar.gz, tar.xz, tar.zst, 7z), GPG encrypt/decrypt/verify, SHA-256 checksum, and Open Terminal Here (foot/alacritty/gnome-terminal/konsole).
- **Nautilus scripts** -- executable scripts in `~/.local/share/nautilus/scripts/` appear in the context menu with Nautilus-compatible environment variables.
- **Compress dialog** -- pick archive name and format from a dialog. Detected formats based on installed tools.

### Integration

- **D-Bus FileManager1** -- Wayfinder registers `org.freedesktop.FileManager1` on the session bus. External apps can call ShowFolders, ShowItems, and ShowItemProperties to open Wayfinder at specific paths.
- **Drag and drop** -- files can be dragged from Wayfinder to other apps (copy/move). Drop files into Wayfinder to copy them to the current directory.
- **Tab path completion** -- press Tab in the location bar to auto-complete paths, with `~` expansion and hidden file awareness.

### File operations

- **Recursive directory copy** -- copying folders now works across filesystems (previously only single files were supported by GIO).
- **Reload after copy/move** -- the directory listing refreshes automatically when a copy or move operation completes.
- **Focus after delete** -- deleting or trashing a file focuses the item above it instead of jumping back to the top.

### Navigation

- **Type-ahead search** -- start typing in the file list to jump to the first matching file. Buffer resets after 800ms. Clears on directory change.

### Accessibility

- **Context menu** -- fully rebuilt as an accessible popover with `Menu` and `MenuItem` roles, arrow key navigation with wrapping, and Right/Left for submenu entry/exit.
- **Properties dialog** -- accessible label ("Properties for {name}").
- **Progress dialogs** -- accessible labels describing the operation.
- **Location bar** -- autocomplete hint for screen readers.
- **Sidebar keyboard** -- Menu key and Shift+F10 open context menus on sidebar items. Bookmark right-click shows "Remove Bookmark" and "Edit Sidebar".

### Internals

- **Symlink-safe folder sizes** -- `symlink_metadata()` prevents following symlinks. Depth guard at 100 prevents infinite recursion.
- **Atomic state persistence** -- window size and sort state use load-modify-save instead of separate writes.
- **O(1) folder size updates** -- HashMap lookup instead of linear scan when updating directory sizes.

---

## 1.2.1

### Fixed

- Context menu items now actually activate. Replaced GMenuModel-based PopoverMenu (whose actions silently failed to resolve) with a manual Popover built from Button widgets.
- Use `GdkAppLaunchContext` instead of `NONE` when launching apps, so child processes inherit the Wayland display environment.

---

## 1.2.0

### Added

- Multi-file selection with Space to toggle and Shift+Space for range selection.
- Location dialog (Ctrl+L) for typing a path directly.
- Portal backend (`wayfinder-portal`) for XDG Desktop Portal file chooser integration.
- Select All (Ctrl+A).

---

## 1.1.0

### Added

- Asynchronous folder size calculation in a background thread.
- Sortable column headers (Name, Size, Modified, Kind) with saved sort state.
- Sidebar with Home, Desktop, Documents, Downloads, Music, Pictures, Videos, File System, and Bin.
- Sidebar toggle (Ctrl+B) with saved visibility state.

### Fixed

- Various stability and performance improvements.

---

## 1.0.0

Initial release.

- GTK4 file manager with list and grid views.
- Full keyboard navigation designed for screen reader users.
- Orca accessibility with `announce()` for state changes.
- File operations: copy, cut, paste, rename, trash, delete, properties.
- Navigation: back, forward, up, path bar.
- Hidden file toggle (Ctrl+H).
- Search (Ctrl+F).
- Per-file app associations via Properties dialog.
