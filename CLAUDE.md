# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build                    # Debug build
cargo build --release          # Optimized build
cargo check                    # Fast type-check without codegen
cargo clippy                   # Lint (must pass with zero warnings)
cargo fmt                      # Format
cargo run                      # Run main file manager
```

Three binaries: `wayfinder` (main app), `wayfinder-portal` (XDG Desktop Portal backend) and `wayfinder-sync` (the Wayfinder Sync user service, below).

```bash
cargo test                     # Unit tests (no display needed)
```

Unit tests cover logic that can run without GTK: path completion (`path_complete.rs`), location naming (`file_model::display_name`), selection after removal (`window/mod.rs`), the suspend-time download retry (`offline/sleep.rs`), and Wayfinder Sync's logic (`cloud/mod.rs`: drives file, sync items, bisync arguments and output, messages). When fixing a bug in logic like this, move it into a plain function and add a test that fails without the fix.

## Architecture

Accessible GTK4 file manager in Rust. Orca screen reader accessibility is the **primary design constraint** — every widget needs `update_property(&[Property::Label(...)])` and state changes should call `window.announce()` (GTK 4.14). No libadwaita dependency; runs on tiled WMs without a desktop environment.

### GObject Two-Module Pattern

Every GObject type (`app/`, `window/`, `file_object/`, `sidebar/`, `views/list_view/`, `views/grid_view/`) uses:
- **`mod.rs`** — `glib::wrapper!` macro, constructor, public API methods
- **`imp.rs`** — `ObjectSubclass` impl, struct fields (usually `RefCell<T>` / `Cell<T>`), `ObjectImpl::constructed()` for UI assembly

Logic-only modules (`file_model/`, `file_ops/`, `navigation/`, `search/`, `clipboard/`, `state.rs`, `actions/`, `context_menu/`, `shortcuts/`, `dbus/`) have no `imp.rs`. Note: `dbus/`, `shortcuts/`, `context_menu/`, `app/`, `window/`, and `portal/` are binary-internal (not re-exported via `lib.rs`).

### Data Flow

```
gio::File → enumerate_children (worker thread, file_model/loader.rs) → FileInfoData → ListStore<FileObject>
  → SortListModel (dirs-first + alpha via CustomSorter)
  → FilterListModel (hidden files via CustomFilter)
  → FilterListModel (search via CustomFilter)
  → SingleSelection → ColumnView / GridView
```

### Threading Model

GObject types (`gio::ListStore`, `gio::FileInfo`, `gtk::CustomSorter`) are **NOT Send**. All GTK/GObject access must happen on the main thread.

**Channel + polling pattern** (used throughout for background → main thread):
1. Spawn `std::thread` for blocking work
2. Send results via `std::sync::mpsc::channel`
3. Poll with `glib::timeout_add_local()` on main thread
4. Construct GObject types only after receiving plain Rust data

This pattern appears in: directory loading, folder size calculation, FUSE async file monitor, D-Bus service, portal backend, custom action completion announcements, compress dialog.

**Directory loading never blocks the main thread.** `DirectoryModel::load_directory(path, resolve, on_done)` lists on a worker thread and waits at most 100 ms (`SYNC_WAIT`) before returning; local folders finish within that, so `on_done` usually runs synchronously. Slower ones finish in the background: `is_loading()` is true meanwhile and the window shows/announces "Waiting for rclone to open X. Press Alt+Left to go back." Every navigation bumps `generation()`, and results of superseded loads are dropped (the blocked syscall can't be cancelled, only abandoned). Callers must not assume the store is populated after `load_directory` returns — put follow-up work in `on_done`. Reloads after file operations use `WayfinderWindow::reload_current()` → `DirectoryModel::refresh(on_done)`, which updates the store in place (diff by path) so the selection survives. `refresh`'s `on_done` always runs, with `true` only if a fresh listing was applied; after removing files, use `reload_after_removal`, which re-selects near them. `DirectoryModel` is `Clone`; clones share state.

**Thread-safe data extraction**: `FileInfoData` struct (plain `String`/`u64`/`i64`/`bool`) is extracted from `gio::FileInfo` on worker threads, sent through channels, then converted to `FileObject` on the main thread via `FileObject::from_data()`.

### FUSE Mount Handling

`is_fuse_mount()` reads `/proc/mounts` to detect rclone, sshfs, etc. (the worker re-checks on the canonical path, so symlinks into a mount count). Never stat, open or list a FUSE path on the main thread: a slow or frozen daemon freezes the window and Orca with it. On FUSE paths:
- Listings request `standard::fast-content-type` instead of `standard::*` — content sniffing would read (download) every ambiguous file
- Listings are cached (`loader.rs`, 128 folders); a cached folder is shown at once and revalidated in the background. Resting the selection on a folder for 400 ms prefetches its listing
- Opening a file picks the app from the name-based MIME type, without a permissions check or content sniffing
- Folder size calculation is **skipped** (shows "—"), and the local size walker never enters FUSE/network mountpoints (matched by path before any stat)
- No file monitor (its path lookup would run on the main thread, and remote changes never reach it); the window calls `refresh_if_idle()` whenever it becomes active instead
- One-off actions (rename, new folder, trash, properties, pinning, paste clash check, Tab completion) go through `wayfinder::background::run` / `WayfinderWindow::run_blocking`, which also announce "Waiting for rclone." when slow
- `trash_or_delete()` returns `NeedsPermanentDelete` when trash isn't supported, triggering a confirmation dialog
- **Changing an rclone mount directly needs its remote control.** rclone caches each folder's listing (`--dir-cache-time`, 5 min) and Proton Drive's backend has no change notifications, so a change made by another rclone process (`rclone moveto`, `purge`, `bisync`) is invisible through the mount until the cache expires. Operations that change a mount go direct only if `rclone_ops::can_change_directly()` (the mount runs `--rc` with `--rc-no-auth` or a login Wayfinder can read from its args or environment); the rclone ops then call `forget_in_mounts()` on the changed folders before their `on_complete`. Otherwise they go through the mount (a move within one mount is still server-side there). Copying *off* a mount changes nothing on it and may always go direct.

### Wayfinder Sync

`wayfinder-sync` (`src/sync_service/main.rs`) is a systemd user service (`~/.config/systemd/user/wayfinder-sync.service`, installed by Wayfinder's setup guide in `src/cloud_sync.rs`, offered the first time something is kept offline on an rclone drive). Drives are in `~/.config/wayfinder/drives` (`cloud::Drive`: remote top level, mountpoint, whether the service runs the mount, rc port). Everything kept offline on such a drive — plus older two-way sync pairings whose local side is an offline copy — is a `cloud::SyncItem`, kept in step with `rclone bisync` (folders) or a newer-wins `copyto` (single files): a few seconds after its offline copy changes (inotify), every five minutes (Proton can't announce changes), on `org.wayfinder.Sync.SyncNow`/`CheckNow` from Wayfinder, and when its drive is mounted again. One sync at a time. Results go out as the `Synced(item, message)` signal (Wayfinder announces it and refreshes windows showing the item) or, with Wayfinder closed, a desktop notification. A managed mount runs as the service's child with `--rc` and a per-run login in its environment, so `forget_in_mounts` can tell it what the sync changed. On such drives offline copies may be changed in Wayfinder (`refuse_in_offline_view` allows it), `offline::sync_all` leaves their pins to the service, and opening a file kept offline opens its offline copy rather than streaming it through the mount (`cloud_sync::file_to_open` → `offline::copy_to_open`, only once the copy is complete).

Gotchas (each found the hard way):
- bisync names its state files after both of its paths joined together; an offline copy's path repeats the original's in full, so deep folders passed the 255-byte file-name limit and every run failed. It's given a short link instead (`cloud::short_local`). rclone aliases don't help: the names come from the underlying path.
- A stopped run leaves a lock that refuses all later runs: `--max-lock`.
- Count changes from what bisync *did* (`Copied`/`Deleted` under "Do queued copies to Path1/Path2"), not what it noticed: the run after a conflict sees the winner "new" on both sides and skips it.
- A first sync (`--resync`) deletes nothing and by default lets the local side win: use `--resync-mode newer`, and refresh an existing offline copy (`offline::refresh_pin`, which keeps offline edits) before the baseline, or files deleted on the drive come back. Each item has its own `--workdir` (`cloud::work_dir`), deleted when it stops being kept offline.
- Opening anything in the offline copies goes by its real path (`offline::real_location`), so a session never gets stuck on a cache path; while a copy is shown, `check_back_online` switches to the real folder once it answers.

### Action Registration Pattern

Window actions use `gio::SimpleAction` registered via `window.add_action()`. Shortcuts use `GtkShortcutController` with `NamedAction` pointing to `win.action-name`. All shortcuts are centralized in `shortcuts/mod.rs`. Actions are set up in `window/imp.rs::setup_actions()`.

**Important**: `Ctrl+Shift+<number>` shortcuts do NOT work reliably in GTK4 due to keyboard layout key translation (Shift+1 produces `!` not `1`). `Ctrl+Shift+<letter>` shortcuts may also be unreliable on some layouts. Use function keys (F3, F4, etc.) for toggles instead.

### Focus / Tab Navigation

Tab from the file list focuses navigation buttons (back/forward/up) then the location entry. Shift+Tab from the file list goes to the location entry. Custom Tab handlers are in `connect_list_key_navigation()` and `connect_grid_key_navigation()` (imp.rs).

**GTK4 ListBox Tab gotcha**: GTK4's ListBox internally consumes Tab to cycle between rows. External `EventControllerKey` (even at Capture phase) cannot intercept Tab before ListBox handles it. Do NOT attempt to add Tab navigation out of the sidebar's `places_list` — it will silently fail. The `set_tab_behavior(ListTabBehavior::Leave)` API only exists in GTK 4.18+, and this project targets `v4_14`.

**Window re-focus**: `connect_is_active_notify` in `imp.rs` restores focus to the selected file item when the window regains focus (e.g. after Alt-Tab from another app).

### Toggle Buttons

Sidebar and breadcrumb bar have paired toggle buttons + actions. The button stores the visual state; the action (for keyboard shortcut) flips the button, which triggers its `connect_toggled` callback. This keeps button state and widget visibility in sync. Both persist visibility via `state.rs`.

### Custom Actions System

`.desktop` files loaded from 6 directories (user → system priority). Format: `[Desktop Entry]` with `Name=`, `Exec=` (%f/%F/%u substitution), `TryExec=` (hides if command missing), `MimeType=`. First match wins for deduplication. All actions (including compress) run in a background thread with a pulsing progress dialog shown via `show_action_progress()`. The dialog closes on completion and announces success/failure via screen reader. Context menu actions pass all selected files (via `get_selected_files()`), so multi-file compress/extract works correctly.

### Menu Bar, Preferences, Operations

`menubar.rs` builds the application's menu bar (`app.set_menubar`; F10 opens it) from the window's `win.` actions plus `app.about`, `app.preferences` and `app.quit`. Items show their shortcut through the `accel` attribute only — the shortcuts themselves stay in `shortcuts/mod.rs` (registering them as accels too would fire them twice). Toggles (`toggle-hidden`, `toggle-sidebar`, `toggle-breadcrumb`) and choices (`view-mode`, `sort-by`, `sort-descending`) are stateful actions, kept in step with the buttons and column headers via `WayfinderWindow::set_action_state`. `preferences.rs` is a Notebook window: General, and the sidebar's places (`sidebar::places_editor`, shared with Edit Sidebar). Progress windows register with `operations` (lib) and get its Continue in Background button (`operations::background_button`), which hides them; Window > Operations in Progress (`win.show-operations`) shows hidden ones again. Closing one while it runs cancels (each window's own close-request handler, gated on `operations::is_running`; custom actions can't be stopped, so theirs hides). When the operation ends call `operations::finished` (window stays for its own Close) or `operations::close`.

### State Persistence

`~/.config/wayfinder/state` — simple `key=value` text file. `state.rs` provides `save_*`/`load_*` pairs for directory, view mode, hidden files, sidebar, window size, sort state, zoom level, file app associations. `~/.config/wayfinder/windows` stores the session's window locations (one per line). Each window joins the session when created (`join_session`) and its entry is updated and written whenever it opens a location (`note_session_location`); closed windows stay in it, and it's written as it changes because at shutdown the windows are already gone. `activate()` reopens them all unless the "Reopen All Windows at Startup" setting (sidebar menu, `state::load_reopen_windows`) is off.

### Portal Binary

`wayfinder-portal` implements `org.freedesktop.impl.portal.FileChooser` via zbus/tokio. Runs a tokio runtime in a background thread for D-Bus, communicates with the GTK main loop via channels. Shares UI patterns with the main app but is a separate binary with its own `main()`.

### Key Modules

- `window/` — Main window (~3,300 lines across mod.rs/imp.rs): all navigation, file operations, dialogs, breadcrumb, zoom. `paste_from()` uses a `pasting` flag to prevent concurrent paste operations.
- `file_model/` — Directory loading (background jobs and FUSE listing cache in `loader.rs`), sorting, filtering, file monitoring, FUSE detection
- `offline/` — "Keep Available Offline" pins: incremental, atomic (`.wayfinder-part` + rename) refresh at startup; `sleep.rs` holds a logind delay inhibitor and pauses the sync before suspend
- `file_ops/` — Copy/move (`transfer.rs`: everything pasted in one progress window — clashes settled in turn, renames first, then counted (`Tally`) and copied, with item countdown, bytes, time left and speed), trash, delete, rename, conflict resolution (Replace/Skip/Rename), cross-device directory move fallback. `on_complete` callback is guaranteed to fire on all code paths (success, error, cancel, early return).
- `sidebar/` — Places, bookmarks (GTK3 format), volumes (GIO VolumeMonitor), editable order/visibility
- `context_menu/` — Accessible popover menus with arrow-key navigation and custom actions
- `actions/` — Desktop file parser, TryExec detection, Nautilus script compatibility, completion announcements
- `properties/` — File properties dialog with interactive permission editor (3×3 checkbox grid + octal entry)
- `clipboard/` — `thread_local!` global clipboard for cross-window copy/paste

### Dependencies

- **gtk4 0.10** with `v4_14` feature
- **zbus 5** (tokio) + **tokio 1** for D-Bus (portal, FileManager1, logind sleep watcher), **futures-util 0.3** for its signal stream
- **dirs 7**, **chrono 0.4**, **log 0.4**, **env_logger 0.11**, **notify 8** (wayfinder-sync's watcher)
