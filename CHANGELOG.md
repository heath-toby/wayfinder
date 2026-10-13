# Changelog

## 2.5.1

### Fixed

- **Data loss when pasting a folder onto itself** — pasting a folder into the folder it's already in and choosing Replace deleted it (or left it empty). Replacing something with itself, or with the folder that holds it, is now refused, and so is moving something to where it already is.
- **Deleting a folder with a drive mounted inside it** — a permanent delete would have gone on into the mounted drive and deleted what's on it (a whole remote drive, say). It's now refused until the drive is unmounted. Permanent deletes of local files also no longer freeze the window while they run.
- **The offline copy** — while a folder's offline copy is shown (its drive can't be reached), deleting, renaming, pasting or creating files there — or in folders opened from it — is refused: it changed the copy, not the drive the location bar names. If the offline copy can't be opened, that's now said.
- **Drag and drop onto rclone mounts** — dropping files where one of the same name already exists asks Skip, Replace or Rename, as pasting does, instead of overwriting it; a folder can't be dropped into itself.
- **Window-local cut and paste** — Ctrl+Shift+X then Ctrl+Shift+V no longer crashes Wayfinder partway through the move.
- **Two-way sync and offline copies** — "Remove Offline Copy" on something inside a two-way sync folder deleted sync's local copy, so the next sync deleted those files on the remote too. The offline refresh at startup could also overwrite or remove local changes in a sync folder that hadn't been uploaded yet. Offline copies inside a sync folder are now left to the sync: never refreshed one way, deleted on unpin, or downloaded over. Changing a sync's remote or folder now starts it afresh, instead of every sync failing.
- **Delete in the sidebar** — pressing Delete on a place such as Home moved whatever was selected in the file list to the Bin; it now does nothing there (it still removes a bookmark).
- **Right-clicking a bookmark** — removed it at once; it now opens the bookmark menu (Remove Bookmark, Edit Sidebar). Removing a bookmark is announced, and focus goes to a neighbouring place.
- **Copying a folder into itself** — pasting a folder into one of its own subfolders copied it into itself over and over until the disk filled; it's now refused.
- **Overwriting on rclone mounts** — pasting onto an rclone mount where a file or folder of the same name already exists now asks Skip, Replace or Rename, as everywhere else, instead of silently overwriting it; renaming to a name that's taken, or a batch rename that would give two files the same name, is refused.
- **rclone copies and moves** — empty folders are copied and moved too, and moving a folder no longer leaves an empty copy of its folder tree behind. When rclone fails, its error message is read out in full.
- **rclone mounts started with flags** — mounts started with options such as `--daemon` or `-v` before the remote name are recognised, so they get the faster direct rclone operations.
- **Copying folders with links** — a folder containing a link to another folder is copied, the link as a link, instead of the whole copy failing.
- **Copy and move messages** — after "Rename (keep both)", the announcement names the copy's new name; a cancelled copy is recognised as cancelled whatever the system language; and "Waiting for rclone" is said if checking for a clash takes a moment.
- **Responsiveness** — paste, delete and rename on many files no longer run `ps` once per file on the main thread, and emptying the Bin no longer freezes the window.
- **Crash in Go to Location** — pressing Tab after typing a full path inside your home folder (such as `/home/you/Docu`), or after a name with accented letters, could close Wayfinder. Completions now keep the form you typed: `~/…` stays `~/…`, and a full path stays a full path.
- **`~` in Go to Location** — only `~` on its own or `~/…` stands for your home folder, so `~bob` is no longer turned into a path inside it, and a folder next to home such as `/home/youx` is no longer shown as `~x`.
- **Selection after Move to Bin or Delete on a slow mount** — if you have gone to another folder by the time the mount answers, your selection there is left alone instead of jumping. "Folder is now empty" is only said when it's true, and "Moved to Bin" is still announced (after the new folder's "Opened" message rather than over it).
- **Permanent delete after Move to Bin fails** — when a file on a mount can't go to the Bin and you delete it permanently, the selection lands near it instead of at the top of the list, and the selection no longer moves while the "Delete permanently?" question is open.
- **Suspend during an offline refresh** — a download stopped for sleep is fetched again after resume, and the rest of the pinned folder carries on, instead of the refresh stopping there or falling back to copying through the mount. A download that had already finished is kept.
- **Waiting messages** — opening the Bin or Recent Files slowly now says "Waiting for the Bin to open" rather than "trash:"; other locations (such as a folder in the Bin, or `network:`) are named properly too.
- **Permanent delete in a local folder** — focus now lands on the file above the deleted one, rather than on the deleted file itself just before it vanished from the list.
- **Deleted files reappearing** — files moved to the Bin or deleted on a mount while you'd gone to another folder no longer show up again, from the recently-seen list, when you go back. The same goes for files pasted, moved, renamed or created there: you see them as they are now, not under their old names — even if you went back into the folder while the operation was still finishing.
- **Offline copy after unmounting** — going back to a folder on a mount that has since been unmounted shows its "Keep Available Offline" copy again, rather than the folder as last seen and then an empty list.
- **Folders deleted elsewhere** — reopening a recently seen folder on a mount that was deleted on another machine now says it can't be opened (or shows the offline copy), rather than quietly switching to its parent while the location bar still showed the old name.
- **File chooser** — when a recently seen folder turns out to be gone, the file chooser now says so, and keeps its item count up to date when a folder changes.
- **Status bar after a failed refresh** — the item count is brought up to date again, as before 2.5.0, while "Waiting for rclone… Press Alt+Left to go back" stays up for as long as a folder is still opening.
- **Deleted folders reappearing** — going Forward into a folder you've just deleted no longer lists its old contents, and undoing a move to the Bin shows the restored files in their folders straight away.
- **Moved and renamed folders** — going back to a folder you've just moved or renamed on a mount no longer opens its old contents as if it were still there.
- **Move to Bin messages** — when some of the selected files had to be deleted permanently instead, "Moved … to Bin" names a file that really went to the Bin.
- **Cancelled deletes** — cancelling a permanent delete on a mount (or one where nothing could be deleted) no longer moves the selection.
- **Memory** — each file chooser dialog is now freed when it closes; previously the portal kept every one until it quit. A window or file chooser closed while a folder was still opening on a stuck mount stops waiting for it — so a closed window can no longer finish opening that folder later and save it as the folder to start in.
- **Dropping files** — files dragged in from other apps are copied even when their names contain spaces or other special characters, and dropping several files at once copies them all.
- **Tab completion** — Go to Location now says how many entries match (for example "2 matches") instead of "No completions"; `~/.` followed by Tab completes hidden files instead of deleting the dot; and a link to a folder completes with a `/`, ready to go on into it.
- **Pasting over a folder** — after pasting a folder onto one of the same name on a mount, opening it shows what's there now, not what it held before.
- **Bin and Recent Files** — if opening one fails after a wait, the "Waiting for…" message is cleared.
- **`~` with an unusual home setting** — Tab completion of `~/…` works when `$HOME` ends in `/`, or is `/` itself.

- **Permissions** — Apply in Properties no longer removes the setuid, setgid and sticky bits (a shared folder at 2775 stayed 2775, not 775); they can also be set by typing a four-digit mode.
- **Keep Available Offline** — only says "Available offline" when the copy really arrived; a failed, cancelled or skipped download is reported and not marked as pinned. Pinning a single file through rclone now stores it where Wayfinder looks for it (it was put inside a folder of the same name).
- **Custom actions** — with several files selected, an action that takes one file (such as Extract Here or Checksum) now runs on each of them, not just the first; actions are only offered when they suit every selected file; file names containing `%c`, `%i` or `%k`, and URIs with spaces or `#`, are passed correctly; the progress window can be put away with "Continue in background" or Escape (it used to stay up, blocking the window, for as long as the program ran — a terminal, say).
- **Compress** — file names beginning with `-` are no longer mistaken for options, and an existing archive of the same name is reported instead of being added to or replaced.
- **File chooser (portal)** — Back, Forward and Up no longer crash it. The chosen file is passed to the app correctly when its name has spaces, `#`, `%` or accented letters (it could open the wrong file). Saving over an existing file asks first. A file chosen in one folder is no longer returned after moving to another, the selected file is shown and read out, and when choosing a folder, the folder being shown can be chosen. Enter in the file name field saves; Escape cancels.
- **Dates** — Date Modified (and the date in Properties) is shown in local time; it was shown in UTC, off by the time zone.
- **Links to folders** — are listed, sorted and opened as folders, rather than as files that opened in another app (without reading anything through a link onto a mount). Copying a link copies the link — or, onto drives that can't hold links (USB sticks, cloud drives), the file it points to.
- **Right-clicking among marked files** — right-clicking a file that isn't marked acts on that file and clears the marks, instead of acting on the marked files.
- **Custom actions on several files** — an action that takes one file now opens them all at once, instead of each waiting for the previous one to be closed.
- **Bin names** — files moved to the Bin from another drive are shown and read out by their own names, not a long encoded path.
- **Sorting** — sorting by Size puts folders in order once their sizes are worked out; folders stay above files in a descending sort; and sorting by clicking a column's own header keeps the sort buttons (and what Orca reads) and the saved sort in step.
- **Folder sizes** — a folder created while you're looking at its parent gets its size worked out (it said "Calculating..." for good), and the last folder's size can no longer go missing.
- **Row labels** — what Orca reads for a file follows changes to it, such as a download growing, instead of keeping its details from when it was first shown.
- **Renaming on case-sensitive drives** — renaming `Report.txt` to `report.txt` where a separate `report.txt` exists is refused, instead of replacing it.
- **Undo move to Bin** — restores the copy just moved there, not an older one from the same place; restoring a file from the Bin whose name has `#` or `%` in it restores that file.
- **Keyboard** — Shift+Tab, and Tab when Back, Forward and Up are unavailable, no longer leave focus stuck in the file list. Shift+Space range selection can no longer freeze the window in an empty or filtered list.
- **Back from the Bin or Recent Files** — goes back to the folder you came from (it skipped it, or went somewhere else); Up does nothing there.
- **Bin menu after editing the sidebar** — right-clicking the Bin keeps its Open Bin / Empty Bin menu after Edit Sidebar.
- **Edit Sidebar** — each place says whether it's shown or hidden, and Enter or Space shows or hides it.
- **Encrypted rclone config** — rclone is never left waiting for a config password, which could freeze the window (opening Set Up Two-Way Sync, for one) or hang an operation.
- **Saving several files from an app** — the file chooser now lets you pick a folder and saves each file there under its own name (numbered if the name is taken), instead of returning a single file for all of them.
- **Bookmarks** — folders with spaces or accented letters are bookmarked correctly (they were written so that GTK apps, and Wayfinder, opened the wrong folder), and such bookmarks made by other apps open properly.
- **"Show in folder"** — from other apps, it now selects the file, and works for files with spaces or accented letters in their path.
- **Open With per file** — the app chosen for a file whose path contains `=` is remembered.
- **Open With** — pressing Enter on "Open With" moves focus into the submenu.
- **Right-click** — right-clicking a file opens the menu for that file; it used to act on whichever file was selected before.

### Development

- **Unit tests** — `cargo test` now covers path completion, location naming, selection after removing files, the folder cache, dropped files, custom action commands, offline pins, and restarting an offline download after suspend.

## 2.5.0

### FUSE / rclone mount support

- **The window no longer freezes on a slow mount** — folders are listed in the background. If one takes more than a moment, the status bar shows, and Orca says, "Waiting for rclone to open Documents. Press Alt+Left to go back." Going back, or anywhere else, works straight away; the late listing is ignored. Local folders open exactly as before.
- **Much faster listings on FUSE mounts** — Wayfinder no longer reads the start of files to work out their type there (on rclone, that meant downloading part of every file); the type comes from the file name.
- **Instant back, up and revisits** — recently seen FUSE folders are shown at once and quietly refreshed. Resting on a folder for a moment fetches its contents in advance, so opening it is instant.
- **Faster startup** — the last folder is listed once instead of twice, and checking that it still exists no longer holds up the window.
- **Refresh in place** — after copying, moving, renaming or deleting, the list updates without resetting, so the selection stays where it was.
- **Opening files on a mount** — the app is chosen from the file type without waiting on the mount.
- **Everything else stays responsive too** — Rename, batch rename, New Folder, Move to Bin, Properties (and applying permissions), Keep Available Offline, the check for an existing file when pasting, and Tab completion in Go to Location all wait on the mount in the background, saying "Waiting for rclone." if it takes a moment. Pressing Delete again while a move to Bin is still waiting does nothing, rather than trashing the same files twice.
- **Changes are picked up when you come back** — a folder on a mount is checked for changes whenever the Wayfinder window becomes active again, instead of relying on a file monitor (which never sees changes made elsewhere, and set up its watch on the main thread).
- **Folders that disappear** — if a recently seen folder can no longer be opened (deleted, or the mount has gone), Wayfinder says so and falls back to the offline copy where there is one, instead of showing what it held last time.
- **Safe to suspend** — the "Keep Available Offline" refresh pauses before the computer sleeps, and folder-size calculation never walks into a FUSE or network mount. Previously either could leave Wayfinder stuck waiting on rclone, which made suspend fail.
- **Offline copies are refreshed, not re-downloaded** — at startup only changed files are fetched, into a temporary file that replaces the old copy once complete, so an interrupted refresh no longer loses the offline copy. Pins are left alone while their mount is down. Changed files are fetched with `rclone copyto` straight from the remote (or with `cp` through the mount when rclone can't), so Wayfinder never holds a file on the mount open — on FUSE, every program Wayfinder started while one was open had to wait for rclone first, which could stall the window for seconds.

### Fixed

- **Focus starts in the file list** — at startup, and after Go to Location, focus is on the selected file instead of the sidebar's first place.
- **Sorting after changes** — a file whose date or size changes moves to its place when the list is sorted by that column.
- **Opening a file from the command line** — `wayfinder some/file` now opens that file, rather than whichever item happened to be selected in its folder.
- **Recent Files with spaces in their names** — no longer silently left out.
- **File chooser (portal)** — Alt+Left, Alt+Right and Alt+Up now go back, forward and up.

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
