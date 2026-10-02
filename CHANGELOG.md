# Changelog

## 2.4.0

### New features

- **Copy and paste files with other apps** — Ctrl+C, Ctrl+X and the Cut and Copy menu items now put files on the system clipboard in the same formats Nautilus and Dolphin use. Pasting into Signal or WhatsApp attaches the files, and other file managers can paste them, including cut-to-move.
- **Paste files copied in other apps** — Ctrl+V pastes files copied or cut in Nautilus, Dolphin or another copy of Wayfinder. When the system clipboard holds no files, Wayfinder's own clipboard is used as before. The window-local clipboard (Ctrl+Shift+C/X/V) stays private.
- **rclone integration** — "Keep Available Offline" pinning for files on rclone mounts, two-way sync setup with `rclone bisync`, and direct `rclone` copy, move and delete with a streaming progress dialog instead of going through the FUSE mount.
- **Run executables on open** — opening an executable file runs it instead of opening it in a text editor. Scripts run in a terminal so their output is visible.
- **Folder-based custom actions** — custom actions can live in their own subfolder, with `./`-relative `Exec=` and `TryExec=` paths resolved against that folder.

### FUSE / rclone mount support

- **Background deletes on FUSE mounts** — deleting on sshfs and other FUSE mounts runs in a background thread with a progress dialog instead of freezing the window.

### Fixed

- **Copy Path and Copy Name** — now actually reach the clipboard. On Wayland, setting the clipboard after the context menu closed was silently ignored by the compositor.
- **Stale paste** — after another app takes over the clipboard, Ctrl+V no longer brings back files copied earlier in Wayfinder.
- **Cut cleanup** — after pasting a cut, the system clipboard is cleared, so neither Wayfinder nor another file manager tries to move the same files twice.

### Removed

- Unused `clap` and `open` dependencies.

## 2.3.0

### New features

- **Font zoom** — Ctrl+Plus/Equal to zoom in, Ctrl+Minus to zoom out, Ctrl+0 to reset. Range 50%–300%. Persisted across sessions. Announced to screen reader.
- **Breadcrumb path bar** — clickable path segments in the header bar replace the read-only location entry. Each segment navigates to that directory. Auto-scrolls to show the deepest segment. Special URIs (Bin, Recent) show a static label. F4 or header bar button to show/hide, persisted across sessions. Announces show/hide state.
- **Recent files** — "Recent" entry in the sidebar shows up to 50 recently opened local files via GTK RecentManager.
- **Batch rename** (Ctrl+Shift+F2) — select multiple files with Space, then batch rename with find/replace. Live preview shows old → new names. Accessible labels on all controls.
- **Replace/Skip/Rename conflict dialog** — when pasting files that already exist at the destination, a dialog offers Skip, Rename (keep both), or Replace. Replace uses OVERWRITE flag throughout including recursive directory copies.
- **Permission editor** — the Properties dialog now has an interactive 3×3 checkbox grid (Owner/Group/Others × Read/Write/Execute) with bidirectional octal entry and an Apply button. Preserves file type bits.
- **Session restore** — all open windows and their directories are saved on exit and restored on next launch. Ctrl+N windows are included.

### FUSE / rclone mount support

- **FUSE mount detection** — reads `/proc/mounts` to detect rclone, sshfs, and other FUSE filesystems.
- **Async file monitor** — on FUSE paths, file monitor events use a 5-second cancellable timeout instead of blocking the main thread.
- **Folder sizes skipped on FUSE** — directories show "—" instead of triggering recursive stat calls that would hang.
- **Trash fallback** — when trash is not supported (e.g. rclone mounts), offers a confirmation dialog to delete permanently.
- **Cross-device directory move** — moving a directory to a FUSE mount now falls back to recursive copy + delete instead of failing with "not supported".
- **Directory copy to FUSE** — `make_directory_with_parents` handles "already exists" errors gracefully.

### Accessibility

- **Action progress dialogs** — Extract Here, Compress, GPG actions, and all other custom actions now show a progress dialog with a pulsing bar while running. The dialog closes automatically on completion and announces success or failure via screen reader.
- **Paste completion announcements** — copy and move operations now announce "Copied filename" or "Moved filename" when each file completes.
- **Multi-file compress** — selecting multiple files with Space and choosing Compress adds all selected files to a single archive. The compress dialog now receives all selected files, not just the focused one.

### Fixed

- **Single-file archive extraction** — archives containing a single file no longer dump it into the parent directory; a subfolder named after the archive is created.
- **Directory replace overwrites correctly** — choosing "Replace" for an existing directory now deletes the old directory before copying, instead of silently merging contents.
- **FUSE permanent delete reloads directory** — after confirming permanent deletion on a FUSE mount, the directory listing now refreshes immediately.
- **Compress dialog default name** — now uses the file stem (without extension) as the default archive name instead of the full filename.
- **Archive extraction folder name** — dotted filenames like "R.A. Salvatore.7z" now extract to a correctly named folder instead of truncating at the first dot.
- **Undo trash accuracy** — undo buffer now only stores paths that were actually trashed, not files that failed.
- **Sidebar toggle** — changed from Ctrl+Shift+S to F3 for reliability across keyboard layouts. Now announces "Sidebar shown/hidden" via screen reader. Button and shortcut stay in sync.
- **Concurrent paste guard** — pressing Ctrl+V while a paste is already in progress now announces "Paste already in progress" instead of spawning duplicate file operations. The guard clears automatically when all files finish (including on error or cancellation).

### Code quality

- **Empty Bin fix** — `empty_trash` now uses recursive deletion so trashed directories are properly removed, not just empty files.
- **Non-local file safety** — copy/move progress dialog gracefully handles non-local GIO files (e.g. trash:// URIs) instead of panicking.
- **Portal runtime safety** — portal D-Bus response serialization no longer panics on failure.
- **Range selection cleanup** — replaced `unwrap()` on range anchor with `if let Some` pattern in both list and grid views.
- **Dead code removal** — removed unused variable bindings in location dialog setup.
- **Redundant clone removal** — eliminated unnecessary `.clone()` calls in drop targets, context menu key handlers, batch rename, and list view builder.
- **Duplicate builder property** — removed duplicated `show_column_separators(true)` in list view construction.
- **Modern format strings** — inlined all format arguments across the codebase (`format!("{var}")` instead of `format!("{}", var)`).
- **Completion callback reliability** — `copy_with_progress` and `move_with_progress` now always fire their completion callback on error, cancellation, and early return paths, not just on success. Prevents callers' cleanup logic from being skipped.
- **Idiomatic patterns** — replaced `while c.is_some() { c.unwrap()... }` with `while let Some(widget)` in sidebar, unnested or-patterns.
- **Sort performance** — name sorting now uses the cached lowercase `search_string` instead of allocating a new `to_lowercase()` string on every comparison, eliminating ~20,000 allocations per sort on a 1,000-file directory.

### Removed

- **Dual pane mode** — removed due to unreliable GTK keyboard shortcut handling with Shift+number keys across keyboard layouts.

---

## 2.2.3

### Accessibility

- **Empty folder announcements** — opening an empty folder announces "Opened X, folder is empty". Opening an empty Bin announces "Opened Bin, Bin is empty". Trashing or deleting the last file announces "folder is now empty" in the message.

---

## 2.2.2

### Fixed

- **Ctrl+A select all** now works in both list and grid views. Previously intercepted by GTK's built-in handler before reaching Wayfinder's selection logic.
- **Shift+Delete multi-file permanent delete** now works with Space-selected files. Previously only deleted the single focused file.
- **Extract Here** for zip files with no top-level folder now creates a subfolder named after the archive instead of dumping files loose into the parent directory.
- **Escape key** now closes the Rename and Create Folder dialogs (previously required clicking Cancel).
- **Focus restoration** after closing the Rename, Create Folder, and Keyboard Shortcuts dialogs — focus returns to the file list instead of getting lost.
- **Directory reload** after rename and create folder operations as a fallback when the file monitor doesn't catch the change.
- **Crash prevention** — replaced unsafe `.unwrap()` calls in file copy/move operations with safe early returns for edge cases (files with no basename, unmounted paths).

---

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

- **Copy Path** — copies the full file path to the system clipboard for pasting into terminals, text editors, etc. Announces "Copied path: /full/path".
- **Copy Name** — copies just the filename to the system clipboard. Announces "Copied name: filename".

### Undo

- **Undo Trash (Ctrl+Z)** — restores the most recently trashed file(s). Searches the Bin by original path, restores to the original location, and reloads the directory. Announces what was restored.

### Terminal

- **Open Terminal Here (Ctrl+`)** — opens a terminal emulator in the current directory. Auto-detects foot, alacritty, gnome-terminal, or konsole.

### Accessibility

- **Sort order announcements** — clicking a column header now announces "Sorted by Name, ascending" (or descending) via the screen reader.

---

## 2.1.0

### Clipboard

- **Cross-window copy/paste** — Ctrl+C/X/V now uses a global clipboard shared across all Wayfinder windows. Copy in one window, paste in another.
- **Window-local clipboard** — Ctrl+Shift+C/X/V for copy/cut/paste scoped to the current window only. Announcements say "(this window)" to distinguish.

### Keyboard shortcuts

- **Backspace** goes to parent directory (same as Alt+Up).
- **Ctrl+?** (Ctrl+Shift+/) opens a Keyboard Shortcuts window listing every shortcut organised by category: Navigation, File Operations, View, and General. Each entry is read by Orca as "Description: Key".
- **Ctrl+Shift+R** now opens File System (previously Ctrl+Shift+C, which is now window-local copy).

### Sidebar

- **Bookmark reordering** — Ctrl+Up/Down reorders bookmarks directly in the sidebar. Announces "Moved above/below {name}" and "Already at top/bottom" at boundaries. Changes persist to the bookmarks file.

### Fixed

- Backspace shortcut was listed in documentation but never registered.

---

## 2.0.0

A major feature release focused on extensibility, integration, and accessibility.

### Sidebar

- **Bookmarks** — Ctrl+D to bookmark the current folder. Delete key to remove. Compatible with `~/.config/gtk-3.0/bookmarks` (Nautilus/GTK format).
- **Edit Sidebar** — right-click or Menu key to open the editor. Toggle places on/off, reorder with Ctrl+Up/Down and Ctrl+Shift+Home/End. Full screen reader announcements ("Moved above Documents", "Already at top").
- **Volume management** — mounted and unmounted volumes appear in the sidebar with eject buttons. Volumes auto-update via GIO VolumeMonitor signals. Click to mount, eject button or right-click to unmount.

### Custom actions

- **Actions system** — context menu actions loaded from `.desktop` files in `~/.local/share/wayfinder/actions/`, `/usr/share/file-manager/actions/` (FMA standard), and `/usr/share/wayfinder/actions/`.
- **TryExec** — actions are hidden if their command is not installed, so bundled defaults work on any system.
- **Bundled actions** — Extract Here (file-roller/ark/bsdtar), Compress (with format picker: zip, tar.gz, tar.xz, tar.zst, 7z), GPG encrypt/decrypt/verify, SHA-256 checksum, and Open Terminal Here (foot/alacritty/gnome-terminal/konsole).
- **Nautilus scripts** — executable scripts in `~/.local/share/nautilus/scripts/` appear in the context menu with Nautilus-compatible environment variables.
- **Compress dialog** — pick archive name and format from a dialog. Detected formats based on installed tools.

### Integration

- **D-Bus FileManager1** — Wayfinder registers `org.freedesktop.FileManager1` on the session bus. External apps can call ShowFolders, ShowItems, and ShowItemProperties to open Wayfinder at specific paths.
- **Drag and drop** — files can be dragged from Wayfinder to other apps (copy/move). Drop files into Wayfinder to copy them to the current directory.
- **Tab path completion** — press Tab in the location bar to auto-complete paths, with `~` expansion and hidden file awareness.

### File operations

- **Recursive directory copy** — copying folders now works across filesystems (previously only single files were supported by GIO).
- **Reload after copy/move** — the directory listing refreshes automatically when a copy or move operation completes.
- **Focus after delete** — deleting or trashing a file focuses the item above it instead of jumping back to the top.

### Navigation

- **Type-ahead search** — start typing in the file list to jump to the first matching file. Buffer resets after 800ms. Clears on directory change.

### Accessibility

- **Context menu** — fully rebuilt as an accessible popover with `Menu` and `MenuItem` roles, arrow key navigation with wrapping, and Right/Left for submenu entry/exit.
- **Properties dialog** — accessible label ("Properties for {name}").
- **Progress dialogs** — accessible labels describing the operation.
- **Location bar** — autocomplete hint for screen readers.
- **Sidebar keyboard** — Menu key and Shift+F10 open context menus on sidebar items. Bookmark right-click shows "Remove Bookmark" and "Edit Sidebar".

### Internals

- **Symlink-safe folder sizes** — `symlink_metadata()` prevents following symlinks. Depth guard at 100 prevents infinite recursion.
- **Atomic state persistence** — window size and sort state use load-modify-save instead of separate writes.
- **O(1) folder size updates** — HashMap lookup instead of linear scan when updating directory sizes.

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
