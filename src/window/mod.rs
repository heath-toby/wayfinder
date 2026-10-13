mod imp;

use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{AccessibleAnnouncementPriority, Application};

use wayfinder::clipboard::{ClipboardOperation, ClipboardState};
use wayfinder::file_object::FileObject;

pub use imp::ViewMode;

/// Where the files being pasted came from, which decides what a cut clears.
#[derive(Clone, Copy)]
enum PasteSource {
    /// Wayfinder's app-wide clipboard (Ctrl+C / Ctrl+X).
    Global,
    /// The window-local clipboard (Ctrl+Shift+C / Ctrl+Shift+X).
    Local,
    /// Another application, e.g. files copied in Nautilus.
    System,
}

/// RAII guard that clears `pasting` on drop. Captures any panic in the
/// progress callback or worker so the flag can never be stranded.
struct PastingGuard {
    win: WayfinderWindow,
}

impl PastingGuard {
    fn new(win: WayfinderWindow) -> Self {
        win.imp().pasting.set(true);
        Self { win }
    }
}

impl Drop for PastingGuard {
    fn drop(&mut self) {
        self.win.imp().pasting.set(false);
    }
}

glib::wrapper! {
    pub struct WayfinderWindow(ObjectSubclass<imp::WayfinderWindowInner>)
        @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl WayfinderWindow {
    pub fn new(app: &Application) -> Self {
        // Explicitly set accessible-role to ApplicationWindow so Orca and
        // other AT-SPI2 clients recognize this as a proper application window.
        // Without this, the window may default to FILLER role which breaks
        // focus tracking and causes spurious window-activation events.
        glib::Object::builder()
            .property("application", app)
            .property("accessible-role", gtk::AccessibleRole::Window)
            .build()
    }

    /// Navigate to a path, updating history. If the path doesn't exist,
    /// the nearest existing parent directory is opened instead.
    pub fn navigate_to_path(&self, path: &str) {
        self.imp().nav.borrow_mut().navigate_to(PathBuf::from(path));
        self.open_directory(path, true);
    }

    /// Load a directory without modifying history (used by back/forward)
    pub fn load_directory(&self, path: &str) {
        if path.contains("://") {
            // The Bin or Recent Files, in the history.
            self.open_special_uri(path);
        } else {
            self.open_directory(path, false);
        }
    }

    /// Remember the selection in the location being left, and clear search.
    fn leave_location(&self, new_location: &str) {
        let imp = self.imp();
        imp.offline_view.set(false);
        let old_path = imp.model.current_path();
        if !old_path.is_empty() && old_path != new_location {
            if let Some(item) = imp.selection.selected_item() {
                if let Some(file) = item.downcast_ref::<FileObject>() {
                    imp.last_position.borrow_mut().insert(old_path, file.name());
                }
            }
        }

        if imp.model.search.is_active() {
            imp.model.search.clear();
            imp.search_entry.set_text("");
            imp.search_bar.set_search_mode(false);
        }
    }

    /// Show `location` in the location bar, breadcrumb and nav buttons.
    fn show_location(&self, location: &str, can_go_up: bool) {
        let imp = self.imp();
        imp.location_entry.set_text(location);
        self.update_breadcrumb(location);
        imp.back_button
            .set_sensitive(imp.nav.borrow().can_go_back());
        imp.forward_button
            .set_sensitive(imp.nav.borrow().can_go_forward());
        imp.up_button.set_sensitive(can_go_up);
    }

    fn open_directory(&self, path: &str, resolve: bool) {
        self.leave_location(path);
        // Show where we're going straight away, so Back works while a slow
        // mount is still answering.
        self.show_location(path, path != "/");

        let w = self.clone();
        let requested = path.to_string();
        self.imp()
            .model
            .load_directory(path, resolve, move |result| {
                w.finish_open_directory(&requested, result)
            });
        self.notify_if_waiting(path);
    }

    /// If the load just started is still waiting after a moment, say so —
    /// in the status bar and aloud — and how to back out.
    fn notify_if_waiting(&self, location: &str) {
        let w = self.clone();
        self.imp().model.notify_if_waiting(location, move |msg| {
            w.imp().status_label.set_text(&msg);
            w.announce(&msg, AccessibleAnnouncementPriority::Medium);
        });
    }

    fn finish_open_directory(&self, requested: &str, live_result: Result<u32, String>) {
        let imp = self.imp();
        // A folder that no longer exists resolves to its nearest parent.
        let resolved = imp.model.current_path();
        if live_result.is_ok() && resolved != requested {
            imp.nav
                .borrow_mut()
                .replace_current(PathBuf::from(&resolved));
            self.show_location(&resolved, resolved != "/");
        }
        let path = if live_result.is_ok() {
            resolved.as_str()
        } else {
            requested
        };

        // Use the count returned directly by load_directory, NOT the
        // filtered model — the filter chain can momentarily report 0 even
        // when the underlying store is populated, which would falsely
        // trigger cache fallback for ordinary directories.
        let live_count = live_result.as_ref().copied().unwrap_or(0);

        // Decide whether to substitute the offline cache. We want the cache
        // to kick in ONLY for paths that are at-or-inside a known rclone
        // FUSE mountpoint (the user has seen mounted at some point).
        // Otherwise an ordinary parent directory like `/home/<user>` could
        // get hijacked by the cache layout, since the cache mirrors the
        // full path structure.
        let path_is_fuse = is_fuse_mount(path);
        let in_known_mount = wayfinder::offline::path_is_in_known_mountpoint(path);
        let cache = wayfinder::offline::cache_path_for(path);
        let cache_has_items = cache.is_dir()
            && std::fs::read_dir(&cache)
                .map(|d| d.count() > 0)
                .unwrap_or(false);

        // Two triggers, both gated on `in_known_mount`:
        // 1. Live load errored (stale mount, read_dir failed).
        // 2. Live load succeeded with 0 items, path isn't currently FUSE
        //    (i.e. cleanly unmounted), and the cache has content.
        let use_cache = in_known_mount
            && cache_has_items
            && (live_result.is_err() || (live_count == 0 && !path_is_fuse));

        if use_cache {
            let cache_str = cache.to_string_lossy().to_string();
            let w = self.clone();
            let path = path.to_string();
            imp.offline_view.set(true);
            imp.model.load_directory(&cache_str, false, move |result| {
                if let Err(e) = result {
                    w.update_status();
                    w.announce(
                        &format!("Error: could not open the offline copy of {path}: {e}"),
                        AccessibleAnnouncementPriority::High,
                    );
                    return;
                }
                let imp = w.imp();
                let path = path.as_str();
                imp.location_entry.set_text(path);
                w.update_breadcrumb(path);
                imp.back_button
                    .set_sensitive(imp.nav.borrow().can_go_back());
                imp.forward_button
                    .set_sensitive(imp.nav.borrow().can_go_forward());
                let at_root = path == "/";
                imp.up_button.set_sensitive(!at_root);
                wayfinder::state::save_last_directory(path);
                w.update_status();

                let dir_name = PathBuf::from(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());
                let count = imp.model.item_count();
                imp.current_column.set(0);
                imp.file_selection.borrow_mut().clear();
                imp.type_ahead_buffer.borrow_mut().clear();
                imp.selection.set_selected(0);
                w.focus_current_view();

                w.announce(
                    &format!(
                        "Opened {dir_name} from offline cache, {count} items. Mount is currently unreachable."
                    ),
                    AccessibleAnnouncementPriority::Medium,
                );
            });
            return;
        }

        match live_result {
            Ok(_count) => {
                imp.location_entry.set_text(path);
                self.update_breadcrumb(path);

                imp.back_button
                    .set_sensitive(imp.nav.borrow().can_go_back());
                imp.forward_button
                    .set_sensitive(imp.nav.borrow().can_go_forward());

                let at_root = path == "/";
                imp.up_button.set_sensitive(!at_root);

                wayfinder::state::save_last_directory(path);

                self.update_status();

                let dir_name = PathBuf::from(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());

                let count = imp.model.item_count();

                // Reset column, selection state, type-ahead
                imp.current_column.set(0);
                imp.file_selection.borrow_mut().clear();
                imp.type_ahead_buffer.borrow_mut().clear();

                // Restore previous selection for this directory if we've
                // visited it before this session, otherwise start at 0.
                let remembered = imp.last_position.borrow().get(path).cloned();
                let mut selected_pos: u32 = 0;
                if let Some(name) = remembered {
                    let model = &imp.model.filter_model;
                    for i in 0..model.n_items() {
                        if let Some(item) = model.item(i) {
                            if let Some(file) = item.downcast_ref::<FileObject>() {
                                if file.name() == name {
                                    selected_pos = i;
                                    break;
                                }
                            }
                        }
                    }
                }
                imp.selection.set_selected(selected_pos);

                // Announce before focus for empty folders (focus change
                // would otherwise override the announcement)
                if count == 0 {
                    self.announce(
                        &format!("Opened {dir_name}, folder is empty"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }

                self.focus_current_view();

                if count > 0 {
                    self.announce(
                        &format!("Opened {dir_name}, {count} items"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }
            }
            Err(e) => {
                log::error!("Failed to load directory {path}: {e}");
                self.update_status();
                self.announce(
                    &format!("Error: could not open {path}"),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
    }

    /// Open the Bin or Recent Files, as a step in the history (so Back
    /// returns to the folder before it).
    pub fn load_special_uri(&self, uri: &str) {
        self.imp().nav.borrow_mut().navigate_to(PathBuf::from(uri));
        self.open_special_uri(uri);
    }

    fn open_special_uri(&self, uri: &str) {
        self.leave_location(uri);
        self.show_location(uri, false);

        let is_recent = uri == "recent:///";
        let w = self.clone();
        let location = uri.to_string();
        let on_done = move |result: Result<u32, String>| w.finish_special_uri(&location, result);
        if is_recent {
            self.imp().model.load_recent(on_done);
        } else {
            self.imp().model.load_uri(uri, on_done);
        }
        self.notify_if_waiting(uri);
    }

    fn finish_special_uri(&self, uri: &str, result: Result<u32, String>) {
        let imp = self.imp();
        let is_recent = uri == "recent:///";
        let label = if is_recent { "Recent Files" } else { "Bin" };

        match result {
            Ok(_count) => {
                imp.location_entry.set_text(uri);
                self.update_breadcrumb(uri);
                imp.back_button
                    .set_sensitive(imp.nav.borrow().can_go_back());
                imp.forward_button
                    .set_sensitive(imp.nav.borrow().can_go_forward());
                imp.up_button.set_sensitive(false);

                self.update_status();

                let count = imp.model.item_count();

                imp.current_column.set(0);
                imp.file_selection.borrow_mut().clear();
                imp.type_ahead_buffer.borrow_mut().clear();

                // Restore previous selection for this URI if known
                let remembered = imp.last_position.borrow().get(uri).cloned();
                let mut selected_pos: u32 = 0;
                if let Some(name) = remembered {
                    let model = &imp.model.filter_model;
                    for i in 0..model.n_items() {
                        if let Some(item) = model.item(i) {
                            if let Some(file) = item.downcast_ref::<FileObject>() {
                                if file.name() == name {
                                    selected_pos = i;
                                    break;
                                }
                            }
                        }
                    }
                }
                imp.selection.set_selected(selected_pos);

                if count == 0 {
                    self.announce(
                        &format!("Opened {label}, {label} is empty"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }

                self.focus_current_view();

                if count > 0 {
                    self.announce(
                        &format!("Opened {label}, {count} items"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }
            }
            Err(e) => {
                log::error!("Failed to load {uri}: {e}");
                // Replace any "Waiting for …" notice.
                self.update_status();
                self.announce(
                    &format!("Error: could not open {label}: {e}"),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
    }

    /// List the current folder again and update it in place, keeping the
    /// selection; then update the status bar. Doesn't block: on a slow
    /// mount the list catches up when rclone answers.
    pub fn reload_current(&self) {
        let w = self.clone();
        self.imp().model.refresh(move |_| w.update_status());
    }

    /// Files or folders at `paths` were moved or renamed away: drop the
    /// cached listings of the old locations and everything inside them.
    pub fn forget_moved(&self, paths: &[String]) {
        for path in paths {
            self.imp().model.forget_cached_under(path);
        }
    }

    /// After files changed in `folders` (pasted into, moved out of, renamed
    /// in): drop their cached listings, so none is shown out of date — if
    /// the user went elsewhere while a slow mount answered, or the reload
    /// is cut short — and reload the current folder.
    pub fn reload_after_change(&self, folders: &[String]) {
        let model = &self.imp().model;
        for folder in folders {
            model.forget_cached(folder);
        }
        self.reload_current();
    }

    /// Run filesystem `work` that may touch a slow mount (`path` says
    /// which) off the main thread, then `done` with the result. If it takes
    /// a moment, says what it's waiting for — in the status bar and aloud.
    pub fn run_blocking<T: Send + 'static>(
        &self,
        path: &str,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&Self, T) + 'static,
    ) {
        let w = self.clone();
        let w_slow = self.clone();
        let waiting = wayfinder::background::waiting_message(path);
        wayfinder::background::run(
            work,
            move |result| {
                done(&w, result);
                w.update_status();
            },
            move || {
                if let Some(msg) = waiting {
                    w_slow.imp().status_label.set_text(&msg);
                    w_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                }
            },
        );
    }

    pub fn update_status(&self) {
        let imp = self.imp();
        if imp.model.is_loading() {
            // The status bar says what we're waiting for, and how to back
            // out; there's no count to give yet.
            return;
        }
        let count = imp.model.item_count();
        let sel_count = imp.file_selection.borrow().count();
        let mut parts = vec![format!("{} items", count)];
        if sel_count > 0 {
            parts.push(format!("{sel_count} selected"));
        }
        if imp.model.showing_hidden() {
            parts.push("showing hidden".to_string());
        }
        if imp.model.search.is_active() {
            parts.push("filtered".to_string());
        }
        let status = parts.join(", ");
        imp.status_label.set_text(&status);
    }

    /// Get selected files — if multi-selection has files, use those; otherwise use the focused file
    pub fn get_selected_files(&self) -> Vec<FileObject> {
        let imp = self.imp();
        let sel = imp.file_selection.borrow();
        if sel.count() > 0 {
            // Return all files matching the selected paths
            let mut files = Vec::new();
            let model = &imp.model.filter_model;
            for i in 0..model.n_items() {
                if let Some(item) = model.item(i) {
                    if let Some(file) = item.downcast_ref::<FileObject>() {
                        if sel.is_selected(&file.path()) {
                            files.push(file.clone());
                        }
                    }
                }
            }
            files
        } else if let Some(file) = self.get_selected_file() {
            vec![file]
        } else {
            vec![]
        }
    }

    pub fn switch_view(&self, mode: ViewMode) {
        let imp = self.imp();
        if imp.current_view.get() == mode {
            return;
        }
        imp.current_view.set(mode);
        match mode {
            ViewMode::Grid => {
                imp.list_view
                    .column_view()
                    .set_model(gtk::SelectionModel::NONE);
                imp.grid_view.set_model(&imp.selection);
                imp.view_stack.set_visible_child_name("grid");
                wayfinder::state::save_view_mode("grid");
                self.announce(
                    "Switched to grid view",
                    AccessibleAnnouncementPriority::Medium,
                );
            }
            ViewMode::List => {
                imp.grid_view
                    .grid_view()
                    .set_model(gtk::SelectionModel::NONE);
                imp.list_view.set_model(&imp.selection);
                imp.view_stack.set_visible_child_name("list");
                wayfinder::state::save_view_mode("list");
                self.announce(
                    "Switched to list view",
                    AccessibleAnnouncementPriority::Medium,
                );
            }
        }
        self.focus_current_view();
    }

    pub fn focus_current_view(&self) {
        let imp = self.imp();
        match imp.current_view.get() {
            ViewMode::Grid => {
                // Use plain grab_focus for grid — scroll_to with FOCUS breaks
                // GtkGridView's internal focus tracking after directory changes
                imp.grid_view.grab_focus();
            }
            ViewMode::List => {
                let pos = imp.selection.selected();
                if pos != gtk::INVALID_LIST_POSITION {
                    imp.list_view.grab_focus_at_selected(pos);
                } else {
                    imp.list_view.grab_focus();
                }
            }
        }
    }

    /// Restore focus to the currently selected item — safe to use when
    /// the directory hasn't changed (e.g. after closing a popover or dialog)
    pub fn restore_focus_to_selected(&self) {
        let imp = self.imp();
        let pos = imp.selection.selected();
        if pos == gtk::INVALID_LIST_POSITION {
            self.focus_current_view();
            return;
        }
        match imp.current_view.get() {
            ViewMode::Grid => {
                imp.grid_view.grab_focus_at_selected(pos);
            }
            ViewMode::List => {
                imp.list_view.grab_focus_at_selected(pos);
            }
        }
    }

    // -- File operations --

    /// Open a file, checking per-file app association first, then MIME default
    pub fn open_file(&self, file: &FileObject) {
        let path = file.path();

        // Check for per-file app association (always takes priority)
        if let Some(desktop_id) = wayfinder::state::load_file_app(&path) {
            let all_apps = gio::AppInfo::all();
            if let Some(app) = all_apps
                .iter()
                .find(|a| a.id().map(|id| id.to_string()) == Some(desktop_id.clone()))
            {
                let gio_file = gio::File::for_path(&path);
                let ctx = WidgetExt::display(self).app_launch_context();
                if let Err(e) = app.launch(&[gio_file], Some(&ctx)) {
                    log::error!("Failed to open with preferred app: {e}");
                    // Fall through to default
                } else {
                    return;
                }
            }
        }

        // On a FUSE mount, choose the app from the type already guessed from
        // the name: checking the permissions or sniffing the contents would
        // wait on the mount with the window frozen. (rclone doesn't mark
        // files executable anyway.)
        // (The model's flag also catches a folder reached through a symlink.)
        if is_fuse_mount(&path) || self.imp().model.is_fuse() {
            let ctx = WidgetExt::display(self).app_launch_context();
            let gio_file = gio::File::for_path(&path);
            let app = gio::AppInfo::default_for_type(&file.mime_type(), false);
            let launched = match app {
                Some(app) => app
                    .launch(&[gio_file], Some(&ctx))
                    .map_err(|e| e.to_string()),
                None => Err("no application for this type".to_string()),
            };
            if let Err(e) = launched {
                log::error!("Failed to open {path}: {e}");
                self.announce(
                    &format!("Failed to open {}", file.name()),
                    AccessibleAnnouncementPriority::High,
                );
            }
            return;
        }

        // If the file is executable, run it directly instead of opening
        // it with a text editor (which is GIO's default for scripts).
        if is_executable_file(&path) {
            match self.execute_file(&path, &file.name()) {
                Ok(()) => return,
                Err(e) => {
                    log::warn!("Failed to execute {path}: {e}. Falling back to open.");
                }
            }
        }

        // Fall back to MIME type default
        let gio_file = gio::File::for_path(&path);
        let uri = gio_file.uri();
        let ctx = WidgetExt::display(self).app_launch_context();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, Some(&ctx)) {
            log::error!("Failed to open {path}: {e}");
            self.announce(
                &format!("Failed to open {}", file.name()),
                AccessibleAnnouncementPriority::High,
            );
        }
    }

    /// Execute a file. Shell scripts and text-based executables are run in
    /// a terminal so the user can see output. Binary executables are spawned
    /// directly.
    fn execute_file(&self, path: &str, name: &str) -> Result<(), String> {
        let is_text = is_text_executable(path);

        if is_text {
            // Text script — run in a terminal so output is visible
            let terminals: &[(&str, &[&str])] = &[
                ("foot", &["-e"]),
                ("alacritty", &["-e"]),
                ("gnome-terminal", &["--"]),
                ("konsole", &["-e"]),
                ("xterm", &["-e"]),
            ];
            for (cmd, args) in terminals {
                if std::process::Command::new("which")
                    .arg(cmd)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
                {
                    // Wrap the script in a shell that keeps the terminal
                    // open after the script exits so output stays visible.
                    let wrapped = format!(
                        "{} ; echo ; echo \"[Press Enter to close]\" ; read",
                        shell_quote(path)
                    );
                    let mut cmd_args: Vec<&str> = args.to_vec();
                    cmd_args.push("sh");
                    cmd_args.push("-c");
                    cmd_args.push(&wrapped);
                    match std::process::Command::new(cmd).args(&cmd_args).spawn() {
                        Ok(_) => {
                            self.announce(
                                &format!("Running {name} in terminal"),
                                AccessibleAnnouncementPriority::Medium,
                            );
                            return Ok(());
                        }
                        Err(e) => {
                            return Err(format!("failed to spawn {cmd}: {e}"));
                        }
                    }
                }
            }
            Err("no terminal emulator found".to_string())
        } else {
            // Binary executable — spawn directly
            match std::process::Command::new(path).spawn() {
                Ok(_) => {
                    self.announce(
                        &format!("Running {name}"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    Ok(())
                }
                Err(e) => Err(format!("spawn failed: {e}")),
            }
        }
    }

    pub fn active_view_widget(&self) -> gtk::Widget {
        let imp = self.imp();
        match imp.current_view.get() {
            ViewMode::Grid => imp.grid_view.grid_view().clone().upcast(),
            ViewMode::List => imp.list_view.column_view().clone().upcast(),
        }
    }

    /// Select `name` the next time `folder` is opened.
    pub fn remember_selection(&self, folder: &str, name: &str) {
        self.imp()
            .last_position
            .borrow_mut()
            .insert(folder.to_string(), name.to_string());
    }

    pub fn get_selected_file(&self) -> Option<FileObject> {
        self.imp().selection.selected_item().and_downcast()
    }

    pub fn is_in_trash(&self) -> bool {
        self.imp().model.current_path().starts_with("trash:")
    }

    pub fn restore_selected(&self) {
        let Some(file) = self.get_selected_file() else {
            return;
        };
        let imp = self.imp();
        let old_pos = imp.selection.selected();

        // The Bin's own name for it (the shown name may differ: see
        // FileInfoData::display_name). child() escapes it: `#` or `%41` in
        // it must stay part of it.
        let trash_name = file
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let gio_file = gio::File::for_uri("trash:///").child(trash_name);
        match wayfinder::file_ops::restore_from_trash(&gio_file) {
            Ok(dest) => {
                // Reload the trash listing so the restored item disappears
                if self.is_in_trash() {
                    self.load_special_uri("trash:///");
                    // Focus the item near where the restored one was
                    let n_items = imp.selection.n_items();
                    if n_items > 0 {
                        let new_pos = if old_pos >= n_items {
                            n_items - 1
                        } else {
                            old_pos
                        };
                        imp.selection.set_selected(new_pos);
                        self.restore_focus_to_selected();
                    }
                }
                self.announce(
                    &format!("Restored {} to {}", file.name(), dest),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
            Err(e) => {
                self.announce(
                    &format!("Failed to restore {}: {}", file.name(), e),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
    }

    pub fn empty_trash(&self) {
        let window = self.clone();
        let dialog = gtk::AlertDialog::builder()
            .message("Empty Bin?")
            .detail("All items in the Bin will be permanently deleted.")
            .buttons(["Cancel", "Empty Bin"])
            .cancel_button(0)
            .default_button(0)
            .build();

        dialog.choose(
            Some(&window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(choice) = result {
                    if choice == 1 {
                        // A big Bin, or one on a slow mount, takes a while:
                        // not on the main thread.
                        window.run_blocking(
                            "trash:///",
                            || wayfinder::file_ops::empty_trash().map_err(|e| e.to_string()),
                            |window, result| match result {
                                Ok(count) => {
                                    window.announce(
                                        &format!("Bin emptied, {count} items deleted"),
                                        AccessibleAnnouncementPriority::Medium,
                                    );
                                    // Reload if we're viewing trash
                                    if window.is_in_trash() {
                                        window.load_special_uri("trash:///");
                                    }
                                }
                                Err(e) => {
                                    window.announce(
                                        &format!("Failed to empty bin: {e}"),
                                        AccessibleAnnouncementPriority::High,
                                    );
                                }
                            },
                        );
                    }
                }
            },
        );
    }

    pub fn show_properties(&self) {
        if let Some(file) = self.get_selected_file() {
            let parent: gtk::Window = self.clone().upcast();
            crate::properties::show_properties_dialog(&file, &parent);
        }
    }

    /// Copy selected files to the global (cross-window) clipboard.
    pub fn copy_selected(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let gio_files: Vec<_> = files
            .iter()
            .map(|f| gio::File::for_path(f.path()))
            .collect();
        let count = gio_files.len();
        let state = ClipboardState::new(ClipboardOperation::Copy, gio_files);
        wayfinder::clipboard::publish(&WidgetExt::display(self), &state);
        wayfinder::clipboard::global_set(state);
        if count == 1 {
            self.announce(
                &format!("Copied {}", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Copied {count} files"),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Cut selected files to the global (cross-window) clipboard.
    pub fn cut_selected(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let gio_files: Vec<_> = files
            .iter()
            .map(|f| gio::File::for_path(f.path()))
            .collect();
        let count = gio_files.len();
        let state = ClipboardState::new(ClipboardOperation::Cut, gio_files);
        wayfinder::clipboard::publish(&WidgetExt::display(self), &state);
        wayfinder::clipboard::global_set(state);
        if count == 1 {
            self.announce(
                &format!("Cut {}", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Cut {count} files"),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Paste from the global (cross-window) clipboard.
    /// Files another app put on the system clipboard (e.g. copied in Nautilus)
    /// take priority. If Wayfinder owns the clipboard, or it holds no files,
    /// use the internal one.
    pub fn paste(&self) {
        let w = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let display = WidgetExt::display(&w);
            match wayfinder::clipboard::read_system(&display).await {
                Some(state) => w.paste_from(Some(state), PasteSource::System),
                None => w.paste_from(wayfinder::clipboard::global_get(), PasteSource::Global),
            }
        });
    }

    /// Copy selected files to the window-local clipboard.
    pub fn copy_selected_local(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let gio_files: Vec<_> = files
            .iter()
            .map(|f| gio::File::for_path(f.path()))
            .collect();
        let count = gio_files.len();
        *self.imp().clipboard.borrow_mut() =
            Some(ClipboardState::new(ClipboardOperation::Copy, gio_files));
        if count == 1 {
            self.announce(
                &format!("Copied {} (this window)", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Copied {count} files (this window)"),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Cut selected files to the window-local clipboard.
    pub fn cut_selected_local(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let gio_files: Vec<_> = files
            .iter()
            .map(|f| gio::File::for_path(f.path()))
            .collect();
        let count = gio_files.len();
        *self.imp().clipboard.borrow_mut() =
            Some(ClipboardState::new(ClipboardOperation::Cut, gio_files));
        if count == 1 {
            self.announce(
                &format!("Cut {} (this window)", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Cut {count} files (this window)"),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Paste from the window-local clipboard.
    pub fn paste_local(&self) {
        // Bound first: a borrow held for the call would still be alive when
        // a cut clears the clipboard at the end of the paste (a panic).
        let clipboard = self.imp().clipboard.borrow().clone();
        self.paste_from(clipboard, PasteSource::Local);
    }

    fn paste_from(&self, clipboard: Option<ClipboardState>, source: PasteSource) {
        if self.refuse_in_offline_view() {
            return;
        }
        let imp = self.imp();

        if imp.pasting.get() {
            self.announce(
                "A paste is already in progress",
                AccessibleAnnouncementPriority::Medium,
            );
            return;
        }

        let Some(state) = clipboard else {
            self.announce("Nothing to paste", AccessibleAnnouncementPriority::Medium);
            return;
        };
        if state.files.is_empty() {
            self.announce("Nothing to paste", AccessibleAnnouncementPriority::Medium);
            return;
        }

        let dest_dir_str = imp.model.current_path();

        // If any source or the destination is on a recognised rclone mount,
        // batch the whole paste through rclone-direct so we get server-side
        // moves where possible and a single progress dialog instead of one
        // per file.
        let source_paths: Vec<String> = state
            .files
            .iter()
            .filter_map(|f| f.path().map(|p| p.to_string_lossy().to_string()))
            .collect();
        let any_src_rclone = source_paths
            .iter()
            .any(|p| wayfinder::rclone_ops::rclone_for_path(p).is_some());
        let dest_rclone = wayfinder::rclone_ops::rclone_for_path(&dest_dir_str).is_some();
        let use_rclone = (any_src_rclone || dest_rclone)
            && wayfinder::rclone_ops::rclone_available()
            && !source_paths.is_empty();

        // The folders this paste changes: the destination, and for a move
        // the folders the files leave.
        let mut changed = vec![dest_dir_str.clone()];
        // Locations whose cached listings (and everything inside) are now
        // stale: what a move takes away, and what the paste lands on — a
        // folder of the same name may be merged into or replaced.
        let mut moved: Vec<String> = landing_paths(&source_paths, &dest_dir_str);
        if state.operation == ClipboardOperation::Cut {
            moved.extend(source_paths.iter().cloned());
            for p in &source_paths {
                if let Some(parent) = parent_dir(p) {
                    if !changed.contains(&parent) {
                        changed.push(parent);
                    }
                }
            }
        }

        // Guard is set immediately and cleared via Drop, so it survives
        // panics in the progress callback or worker.
        let guard = PastingGuard::new(self.clone());

        let plan = PastePlan {
            files: state.files.clone(),
            source_paths,
            dest_dir: dest_dir_str,
            is_move: state.operation == ClipboardOperation::Cut,
            changed,
            moved,
        };
        if !self.dispatch_paste(plan, guard, use_rclone) {
            return;
        }

        // Clear clipboard after cut
        if state.operation == ClipboardOperation::Cut {
            match source {
                PasteSource::Global => {
                    wayfinder::clipboard::global_clear();
                    // Withdraw the cut list we published, unless another
                    // app has taken over the clipboard since.
                    let clipboard = WidgetExt::display(self).clipboard();
                    if clipboard.is_local() {
                        let _ = clipboard.set_content(None::<&gtk::gdk::ContentProvider>);
                    }
                }
                PasteSource::Local => *imp.clipboard.borrow_mut() = None,
                // The files have moved, so the other app's list is stale.
                // Clear it as Nautilus does, so a second paste can't fail.
                PasteSource::System => {
                    let _ = WidgetExt::display(self)
                        .clipboard()
                        .set_content(None::<&gtk::gdk::ContentProvider>);
                }
            }
        }
    }

    /// Start a paste (or drop). Returns false if it was refused.
    fn dispatch_paste(&self, plan: PastePlan, guard: PastingGuard, use_rclone: bool) -> bool {
        // rclone doesn't refuse these: a folder into itself, or a move to
        // where it already is. (The per-file way checks for itself.)
        if use_rclone {
            use wayfinder::file_ops::{placement, Placement};
            for (src, dest) in plan
                .source_paths
                .iter()
                .zip(landing_paths(&plan.source_paths, &plan.dest_dir))
            {
                let name = std::path::Path::new(src)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let msg = match placement(std::path::Path::new(src), std::path::Path::new(&dest)) {
                    Placement::IntoItself => Some(format!(
                        "Cannot {} {name} into itself",
                        if plan.is_move { "move" } else { "copy" }
                    )),
                    Placement::Same if plan.is_move => Some(format!("{name} is already here")),
                    _ => None,
                };
                if let Some(msg) = msg {
                    self.announce(&msg, AccessibleAnnouncementPriority::High);
                    return false;
                }
            }

            // rclone overwrites (or merges into) whatever is in the way
            // without asking. If anything is, go the per-file way, which
            // asks Skip, Replace or Rename.
            let landing = landing_paths(&plan.source_paths, &plan.dest_dir);
            self.run_blocking(
                &plan.dest_dir.clone(),
                move || landing.iter().any(|p| std::path::Path::new(p).exists()),
                move |w, clash| {
                    if clash {
                        w.paste_with_gio(plan, guard);
                    } else {
                        w.paste_with_rclone(plan, guard);
                    }
                },
            );
        } else {
            self.paste_with_gio(plan, guard);
        }
        true
    }

    /// Paste through rclone directly: one progress dialog, server-side
    /// moves where possible. Only when nothing is in the way.
    fn paste_with_rclone(&self, plan: PastePlan, guard: PastingGuard) {
        let parent_window: gtk::Window = self.clone().upcast();
        let w = self.clone();
        let (changed, moved) = (plan.changed, plan.moved);
        let reload: Option<Box<dyn FnOnce() + 'static>> = Some(Box::new(move || {
            // Move guard into the closure so it drops here on success.
            let _g = guard;
            w.forget_moved(&moved);
            w.reload_after_change(&changed);
        }));
        if plan.is_move {
            wayfinder::rclone_ops::move_paths_with_progress(
                plan.source_paths,
                plan.dest_dir,
                &parent_window,
                reload,
            );
        } else {
            wayfinder::rclone_ops::copy_paths_with_progress(
                plan.source_paths,
                plan.dest_dir,
                &parent_window,
                reload,
            );
        }
    }

    /// Paste file by file through GIO, asking about each clash.
    fn paste_with_gio(&self, plan: PastePlan, guard: PastingGuard) {
        let parent_window: gtk::Window = self.clone().upcast();
        let dest_dir_gio = gio::File::for_path(&plan.dest_dir);
        // Each reload callback holds a clone of the shared guard, which
        // drops when the last one finishes (or all are dropped without
        // running, e.g. due to panic).
        let shared_guard = std::rc::Rc::new(guard);
        for source in &plan.files {
            let w = self.clone();
            let guard_held = shared_guard.clone();
            let changed = plan.changed.clone();
            let moved = plan.moved.clone();
            let reload: Option<Box<dyn FnOnce() + 'static>> = Some(Box::new(move || {
                let _g = guard_held;
                w.forget_moved(&moved);
                w.reload_after_change(&changed);
            }));
            if plan.is_move {
                wayfinder::file_ops::move_with_progress(
                    source,
                    &dest_dir_gio,
                    &parent_window,
                    reload,
                );
            } else {
                wayfinder::file_ops::copy_with_progress(
                    source,
                    &dest_dir_gio,
                    &parent_window,
                    reload,
                );
            }
        }
    }

    pub fn trash_selected(&self) {
        if self.refuse_in_offline_view() {
            return;
        }
        // Pressing Delete again while a slow mount is still answering would
        // trash (or offer to permanently delete) the same files twice, so
        // skip files already on their way.
        let files: Vec<FileObject> = {
            let trashing = self.imp().trashing.borrow();
            self.get_selected_files()
                .into_iter()
                .filter(|f| !trashing.contains(&f.path()))
                .collect()
        };
        if files.is_empty() {
            return;
        }

        let from = RemovedFrom::here(self);

        let paths: Vec<String> = files.iter().map(|f| f.path()).collect();
        self.imp()
            .trashing
            .borrow_mut()
            .extend(paths.iter().cloned());
        let count = paths.len();
        self.run_blocking(
            &paths[0].clone(),
            move || {
                // A panic would drop the result and leave these paths
                // marked as in progress for good.
                std::panic::catch_unwind(|| {
                    paths
                        .iter()
                        .map(|p| {
                            wayfinder::file_ops::trash_or_delete(&gio::File::for_path(p))
                                .map_err(|e| e.to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|_| {
                    (0..count)
                        .map(|_| Err("internal error".to_string()))
                        .collect()
                })
            },
            move |w, results| {
                {
                    let mut trashing = w.imp().trashing.borrow_mut();
                    for f in &files {
                        trashing.remove(&f.path());
                    }
                }
                w.finish_trash(files, from, results)
            },
        );
    }

    fn finish_trash(
        &self,
        files: Vec<FileObject>,
        from: RemovedFrom,
        results: Vec<Result<wayfinder::file_ops::TrashResult, String>>,
    ) {
        let imp = self.imp();
        let mut success = 0;
        let mut failed = 0;
        let mut last_error = String::new();
        let mut needs_perm_delete: Vec<FileObject> = Vec::new();
        let mut trashed_paths: Vec<String> = Vec::new();

        for (file, result) in files.iter().zip(results) {
            match result {
                Ok(wayfinder::file_ops::TrashResult::Trashed) => {
                    success += 1;
                    trashed_paths.push(file.path());
                }
                Ok(wayfinder::file_ops::TrashResult::NeedsPermanentDelete) => {
                    needs_perm_delete.push(file.clone());
                }
                Err(e) => {
                    failed += 1;
                    last_error = format!("{}: {}", file.name(), e);
                }
            }
        }

        // Reloading and re-selecting for the files that did go to the Bin
        // waits while "Delete permanently?" is open, so nothing moves
        // behind the question; it runs once that's answered.
        let after_question: Deferred = Default::default();

        // If some files can't be trashed (e.g. FUSE mount), offer permanent
        // deletion — unless a drive is mounted inside one of them.
        let perm_paths: Vec<String> = needs_perm_delete.iter().map(|f| f.path()).collect();
        if !needs_perm_delete.is_empty() && self.refuse_if_mounts_inside(&perm_paths) {
            needs_perm_delete.clear();
        }
        if !needs_perm_delete.is_empty() {
            let window = self.clone();
            let count = needs_perm_delete.len();
            let message = if count == 1 {
                format!(
                    "{} is on a remote mount and cannot be moved to Bin. Delete permanently?",
                    needs_perm_delete[0].name()
                )
            } else {
                format!(
                    "{count} files are on a remote mount and cannot be moved to Bin. Delete permanently?"
                )
            };

            let dialog = gtk::AlertDialog::builder()
                .message(message)
                .detail("This cannot be undone.")
                .buttons(["Cancel", "Delete permanently"])
                .cancel_button(0)
                .default_button(0)
                .build();

            let file_paths: Vec<(String, String)> = needs_perm_delete
                .iter()
                .map(|f| (f.name(), f.path()))
                .collect();
            let file_path_dirs: Vec<bool> =
                needs_perm_delete.iter().map(|f| f.is_directory()).collect();
            let from = from.clone();
            let after_question = after_question.clone();

            dialog.choose(
                Some(&window.clone()),
                gio::Cancellable::NONE,
                move |result| {
                    if let Some(trashed) = after_question.borrow_mut().take() {
                        trashed();
                    }
                    if let Ok(choice) = result {
                        if choice == 1 {
                            // Prefer rclone-direct delete when every path is on
                            // a recognised rclone mount.
                            let all_rclone = !file_paths.is_empty()
                                && file_paths.iter().all(|(_, p)| {
                                    wayfinder::rclone_ops::rclone_for_path(p).is_some()
                                })
                                && wayfinder::rclone_ops::rclone_available();
                            if all_rclone {
                                let rclone_items: Vec<(String, bool)> = file_paths
                                    .iter()
                                    .zip(file_path_dirs.iter())
                                    .map(|((_, p), d)| (p.clone(), *d))
                                    .collect();
                                run_rclone_direct_delete(&window, rclone_items, from);
                            } else {
                                // Other FUSE mounts: threaded std::fs delete.
                                run_threaded_delete(&window, file_paths, from);
                            }
                        }
                    }
                },
            );
        }

        // Store trashed paths for undo (only successfully trashed ones)
        if !trashed_paths.is_empty() {
            *imp.last_trashed.borrow_mut() = trashed_paths.clone();
        }

        // Announce failures FIRST (before focus changes trigger Orca)
        if failed > 0 {
            if failed == 1 {
                self.announce(
                    &format!("Could not move to Bin: {last_error}"),
                    AccessibleAnnouncementPriority::High,
                );
            } else {
                self.announce(
                    &format!("{failed} files could not be moved to Bin"),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
        if success == 0 {
            // Nothing left the folder; a permanent delete (if confirmed)
            // reloads and selects for itself.
            return;
        }

        // Refresh, then focus the item above the deleted one (or the new
        // last item) and confirm.
        // Name a file that really went to the Bin.
        let first_name = files
            .iter()
            .find(|f| trashed_paths.contains(&f.path()))
            .map(|f| f.name())
            .unwrap_or_default();
        let window = self.clone();
        let finish = move || {
            window.reload_after_removal(from, move |w, relisted| {
                if failed == 0 && success > 0 {
                    // Unless the folder was just listed again, the current
                    // view's emptiness says nothing about it.
                    let now_empty = relisted && w.imp().selection.n_items() == 0;
                    let msg = if success == 1 {
                        if now_empty {
                            format!("Moved {first_name} to Bin, folder is now empty")
                        } else {
                            format!("Moved {first_name} to Bin")
                        }
                    } else if now_empty {
                        format!("Moved {success} files to Bin, folder is now empty")
                    } else {
                        format!("Moved {success} files to Bin")
                    };

                    if relisted {
                        w.announce(&msg, AccessibleAnnouncementPriority::Medium);
                    } else {
                        w.announce_after_navigation(msg);
                    }
                }
            });
        };
        if needs_perm_delete.is_empty() {
            finish();
        } else {
            *after_question.borrow_mut() = Some(Box::new(finish));
        }
    }

    /// After items were removed `from` a folder: list it again, select the
    /// item above the removed one (or the new last item), then run `after`,
    /// told whether that happened. It doesn't if the user went elsewhere
    /// while a slow mount answered (even if they came back), or if a
    /// navigation gets in before the listing; `after` runs either way.
    fn reload_after_removal(&self, from: RemovedFrom, after: impl FnOnce(&Self, bool) + 'static) {
        let model = &self.imp().model;
        // Cached listings still hold the removed items (and a removed
        // folder's own contents); don't show them again, whether the user
        // went elsewhere or the reload below is cut short.
        model.forget_cached(&from.folder);
        for path in &from.removed {
            model.forget_cached_under(path);
            // Their own folders too: in Recent Files that isn't the view.
            if let Some(parent) = parent_dir(path) {
                model.forget_cached(&parent);
            }
        }
        if model.current_path() != from.folder {
            after(self, false);
            return;
        }
        if model.generation() == from.generation {
            self.imp().file_selection.borrow_mut().clear();
            self.update_status();
        }
        let w = self.clone();
        model.refresh(move |listed| {
            w.update_status();
            // Same view of the folder (no navigation since): move off the
            // removed items — even if listing failed and they're still
            // shown, as reloading always did.
            let same_view = w.imp().model.generation() == from.generation;
            if same_view {
                w.select_near_removed(from.old_pos);
            }
            after(&w, listed && same_view);
        });
    }

    /// In the offline copy (mount unreachable), say that files can't be
    /// changed and return true: the change would land on the copy, not the
    /// drive the location bar names.
    fn refuse_in_offline_view(&self) -> bool {
        if !self.imp().offline_view.get() {
            return false;
        }
        self.announce(
            "This is the offline copy, shown while the drive can't be reached. It can't be changed here.",
            AccessibleAnnouncementPriority::High,
        );
        true
    }

    /// If a drive is mounted inside any of `paths`, say so and return true:
    /// deleting them permanently would delete what's on that drive too.
    fn refuse_if_mounts_inside(&self, paths: &[String]) -> bool {
        for path in paths {
            if let Some(mount) = wayfinder::rclone_ops::mountpoints_inside(path).first() {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                self.announce(
                    &format!("Not deleted: {mount} is mounted inside {name}. Unmount it first."),
                    AccessibleAnnouncementPriority::High,
                );
                return true;
            }
        }
        false
    }

    /// Select the item above one removed from `old_pos`, or the new last
    /// item, and focus it.
    fn select_near_removed(&self, old_pos: u32) {
        let imp = self.imp();
        if let Some(pos) = position_near_removed(old_pos, imp.selection.n_items()) {
            imp.selection.set_selected(pos);
            self.restore_focus_to_selected();
        }
    }

    /// Announce `msg` once a navigation under way has finished, so it comes
    /// after that folder's "Opened" announcement instead of with it.
    fn announce_after_navigation(&self, msg: String) {
        let w = self.clone();
        let mut polls = 0;
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            polls += 1;
            // Don't wait forever on a mount that has stopped answering.
            if w.imp().model.is_loading() && polls < 40 {
                return glib::ControlFlow::Continue;
            }
            w.announce(&msg, AccessibleAnnouncementPriority::Medium);
            glib::ControlFlow::Break
        });
    }

    pub fn delete_selected(&self) {
        if self.refuse_in_offline_view() {
            return;
        }
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let paths: Vec<String> = files.iter().map(|f| f.path()).collect();
        if self.refuse_if_mounts_inside(&paths) {
            return;
        }

        let window = self.clone();
        let from = RemovedFrom::here(self);
        let count = files.len();

        let message = if count == 1 {
            format!("Permanently delete {}?", files[0].name())
        } else {
            format!("Permanently delete {count} items?")
        };

        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail("This cannot be undone.")
            .buttons(["Cancel", "Delete permanently"])
            .cancel_button(0)
            .default_button(0)
            .build();

        // Capture paths as strings before the async callback
        let file_paths: Vec<(String, String)> =
            files.iter().map(|f| (f.name(), f.path())).collect();
        let file_path_dirs: Vec<bool> = files.iter().map(|f| f.is_directory()).collect();

        dialog.choose(
            Some(&window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(choice) = result {
                    if choice == 1 {
                        // If every path lives on an rclone FUSE mount, dispatch
                        // direct to rclone — much faster, real progress, proper
                        // rate-limit handling.
                        let all_rclone = !file_paths.is_empty()
                            && file_paths
                                .iter()
                                .all(|(_, p)| wayfinder::rclone_ops::rclone_for_path(p).is_some())
                            && wayfinder::rclone_ops::rclone_available();
                        if all_rclone {
                            let rclone_items: Vec<(String, bool)> = file_paths
                                .iter()
                                .zip(file_path_dirs.iter())
                                .map(|((_, p), d)| (p.clone(), *d))
                                .collect();
                            run_rclone_direct_delete(&window, rclone_items, from);
                            return;
                        }

                        // Otherwise: if any path is a FUSE mount (sshfs, gvfs,
                        // unrecognised rclone mount, etc.), run the delete in
                        // a background thread with a progress dialog so the
                        // UI stays responsive.
                        let any_slow = file_paths.iter().any(|(_, p)| is_fuse_mount(p));
                        if any_slow {
                            run_threaded_delete(&window, file_paths, from);
                            return;
                        }

                        // Off the main thread: a big folder takes a while,
                        // and a link may lead onto a slow mount.
                        let first = file_paths
                            .first()
                            .map(|(_, p)| p.clone())
                            .unwrap_or_default();
                        window.run_blocking(
                            &first,
                            move || {
                                let mut success = 0;
                                let mut failed = 0;
                                let mut last_error = String::new();
                                for (name, path) in &file_paths {
                                    let gio_file = gio::File::for_path(path);
                                    match wayfinder::file_ops::delete_file_recursive(&gio_file) {
                                        Ok(()) => success += 1,
                                        Err(e) => {
                                            failed += 1;
                                            last_error = format!("{name}: {e}");
                                        }
                                    }
                                }
                                let first_name = file_paths
                                    .first()
                                    .map(|(n, _)| n.clone())
                                    .unwrap_or_default();
                                (success, failed, last_error, first_name)
                            },
                            move |window, (success, failed, last_error, first_name)| {
                                // Announce failures first
                                if failed > 0 {
                                    if failed == 1 {
                                        window.announce(
                                            &format!("Could not delete: {last_error}"),
                                            AccessibleAnnouncementPriority::High,
                                        );
                                    } else {
                                        window.announce(
                                            &format!("{failed} files could not be deleted"),
                                            AccessibleAnnouncementPriority::High,
                                        );
                                    }
                                }
                                if success > 0 {
                                    // Select once the list no longer holds the
                                    // deleted rows, so focus can't land on one.
                                    window.reload_after_removal(from, move |w, relisted| {
                                        if failed > 0 {
                                            return;
                                        }
                                        let msg = if success == 1 {
                                            format!("Deleted {first_name}")
                                        } else {
                                            format!("Deleted {success} files")
                                        };
                                        if relisted {
                                            w.announce(
                                                &msg,
                                                AccessibleAnnouncementPriority::Medium,
                                            );
                                        } else {
                                            w.announce_after_navigation(msg);
                                        }
                                    });
                                }
                            },
                        );
                    }
                }
            },
        );
    }

    pub fn rename_selected(&self) {
        if self.refuse_in_offline_view() {
            return;
        }
        let Some(file) = self.get_selected_file() else {
            return;
        };

        let window = self.clone();

        let entry = gtk::Entry::builder()
            .text(file.name())
            .hexpand(true)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label(&format!(
            "New name for {}",
            file.name()
        ))]);
        // Select the name without extension for convenience
        if let Some(dot_pos) = file.name().rfind('.') {
            entry.select_region(0, dot_pos as i32);
        } else {
            entry.select_region(0, -1);
        }

        let dlg = gtk::Window::builder()
            .title(format!("Rename {}", file.name()))
            .modal(true)
            .transient_for(&window)
            .default_width(400)
            .build();

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let label = gtk::Label::new(Some(&format!("Rename {}", file.name())));
        vbox.append(&label);
        vbox.append(&entry);

        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        button_box.set_halign(gtk::Align::End);

        let cancel_btn = gtk::Button::with_label("Cancel");
        let rename_btn = gtk::Button::with_label("Rename");
        rename_btn.add_css_class("suggested-action");
        button_box.append(&cancel_btn);
        button_box.append(&rename_btn);
        vbox.append(&button_box);

        dlg.set_child(Some(&vbox));

        let d = dlg.clone();
        cancel_btn.connect_clicked(move |_| {
            d.close();
        });

        let d = dlg.clone();
        let entry_clone = entry.clone();
        let file_path = file.path();
        let w = window.clone();
        let do_rename = move || {
            let new_name = entry_clone.text().to_string();
            if new_name.is_empty() || new_name == file.name() {
                d.close();
                return;
            }

            // rclone-direct rename: server-side moveto, no FUSE round-trip.
            if wayfinder::rclone_ops::rclone_for_path(&file_path).is_some()
                && wayfinder::rclone_ops::rclone_available()
            {
                let parent: gtk::Window = w.clone().upcast();
                let new_path = std::path::Path::new(&file_path)
                    .parent()
                    .map(|p| p.join(&new_name).to_string_lossy().to_string())
                    .unwrap_or_else(|| new_name.clone());
                let changed: Vec<String> = parent_dir(&file_path).into_iter().collect();
                let moved = vec![file_path.clone()];
                let old_path = file_path.clone();
                let old_check = file_path.clone();
                let target = new_path.clone();

                d.close();
                // rclone would overwrite whatever has the new name; GIO's
                // rename refuses, and so do we.
                w.run_blocking(
                    &file_path,
                    move || rename_clash(&old_check, &target),
                    move |w, clash| {
                        if clash {
                            w.announce(
                                &format!("Could not rename: {new_name} already exists"),
                                AccessibleAnnouncementPriority::High,
                            );
                            return;
                        }
                        let w_done = w.clone();
                        let on_complete: Option<Box<dyn FnOnce() + 'static>> =
                            Some(Box::new(move || {
                                w_done.forget_moved(&moved);
                                w_done.reload_after_change(&changed);
                            }));
                        wayfinder::rclone_ops::rename_with_progress(
                            old_path,
                            new_path,
                            &parent,
                            on_complete,
                        );
                    },
                );
                return;
            }

            d.close();
            let path = file_path.clone();
            let changed: Vec<String> = parent_dir(&file_path).into_iter().collect();
            let moved = vec![file_path.clone()];
            w.run_blocking(
                &file_path,
                move || {
                    wayfinder::file_ops::rename_file(&gio::File::for_path(&path), &new_name)
                        .map(|_| new_name)
                        .map_err(|e| e.to_string())
                },
                move |w, result| match result {
                    Ok(new_name) => {
                        w.announce(
                            &format!("Renamed to {new_name}"),
                            AccessibleAnnouncementPriority::Medium,
                        );
                        // Reload directory as fallback in case file monitor doesn't catch the rename
                        w.forget_moved(&moved);
                        w.reload_after_change(&changed);
                    }
                    Err(e) => {
                        w.announce(
                            &format!("Rename failed: {e}"),
                            AccessibleAnnouncementPriority::High,
                        );
                    }
                },
            );
        };

        let do_rename_clone = do_rename.clone();
        rename_btn.connect_clicked(move |_| {
            do_rename_clone();
        });

        entry.connect_activate(move |_| {
            do_rename();
        });

        let key_ctrl = gtk::EventControllerKey::new();
        let d = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);

        let w = window.clone();
        dlg.connect_close_request(move |_| {
            w.restore_focus_to_selected();
            glib::Propagation::Proceed
        });

        dlg.present();
        entry.grab_focus();
    }

    pub fn batch_rename(&self) {
        if self.refuse_in_offline_view() {
            return;
        }
        let files = self.get_selected_files();
        if files.len() < 2 {
            self.announce(
                "Select multiple files to batch rename",
                AccessibleAnnouncementPriority::Medium,
            );
            return;
        }

        let window = self.clone();

        let dlg = gtk::Window::builder()
            .title("Batch Rename")
            .modal(true)
            .transient_for(&window)
            .default_width(500)
            .default_height(400)
            .build();
        dlg.update_property(&[gtk::accessible::Property::Label("Batch rename files")]);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        // Find/Replace entries
        let find_entry = gtk::Entry::builder()
            .placeholder_text("Find text...")
            .hexpand(true)
            .build();
        find_entry.update_property(&[gtk::accessible::Property::Label("Find text in filenames")]);

        let replace_entry = gtk::Entry::builder()
            .placeholder_text("Replace with...")
            .hexpand(true)
            .build();
        replace_entry.update_property(&[gtk::accessible::Property::Label("Replace with text")]);

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(8)
            .build();
        let find_label = gtk::Label::builder().label("Find:").xalign(1.0).build();
        let replace_label = gtk::Label::builder().label("Replace:").xalign(1.0).build();
        grid.attach(&find_label, 0, 0, 1, 1);
        grid.attach(&find_entry, 1, 0, 1, 1);
        grid.attach(&replace_label, 0, 1, 1, 1);
        grid.attach(&replace_entry, 1, 1, 1, 1);
        vbox.append(&grid);

        // Preview list
        let preview_label = gtk::Label::builder()
            .label("Preview:")
            .xalign(0.0)
            .margin_top(8)
            .build();
        vbox.append(&preview_label);

        let preview_list = gtk::ListBox::new();
        preview_list.set_selection_mode(gtk::SelectionMode::None);
        preview_list.update_property(&[gtk::accessible::Property::Label("Rename preview")]);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&preview_list)
            .build();
        vbox.append(&scrolled);

        // Populate initial preview
        let file_names: Vec<String> = files.iter().map(|f| f.name()).collect();
        let file_paths: Vec<String> = files.iter().map(|f| f.path()).collect();

        let names_rc = std::rc::Rc::new(file_names);
        let preview_ref = preview_list.clone();

        let update_preview = {
            let names = names_rc.clone();
            let preview = preview_ref.clone();
            let find = find_entry.clone();
            let replace = replace_entry.clone();
            move || {
                while let Some(child) = preview.first_child() {
                    preview.remove(&child);
                }
                let find_text = find.text().to_string();
                for name in names.iter() {
                    let new_name = if find_text.is_empty() {
                        name.clone()
                    } else {
                        name.replace(&find_text, &replace.text())
                    };
                    let changed = *name != new_name;
                    let display = if changed {
                        format!("{name} → {new_name}")
                    } else {
                        name.clone()
                    };
                    let label = gtk::Label::builder()
                        .label(&display)
                        .xalign(0.0)
                        .margin_start(8)
                        .margin_end(8)
                        .margin_top(2)
                        .margin_bottom(2)
                        .build();
                    if changed {
                        label.add_css_class("accent");
                    }
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&label));
                    row.set_selectable(false);
                    row.update_property(&[gtk::accessible::Property::Label(&display)]);
                    preview.append(&row);
                }
            }
        };

        // Initial preview
        update_preview();

        // Update preview on text change
        let up1 = update_preview.clone();
        find_entry.connect_changed(move |_| up1());
        let up2 = update_preview.clone();
        replace_entry.connect_changed(move |_| up2());

        // Buttons
        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        button_box.set_halign(gtk::Align::End);
        button_box.set_margin_top(8);

        let cancel_btn = gtk::Button::with_label("Cancel");
        let rename_btn = gtk::Button::with_label("Rename All");
        rename_btn.add_css_class("suggested-action");
        rename_btn.update_property(&[gtk::accessible::Property::Label("Rename all files")]);
        button_box.append(&cancel_btn);
        button_box.append(&rename_btn);
        vbox.append(&button_box);

        dlg.set_child(Some(&vbox));

        let d = dlg.clone();
        cancel_btn.connect_clicked(move |_| d.close());

        let d = dlg.clone();
        let w = window.clone();
        let find_e = find_entry.clone();
        let replace_e = replace_entry.clone();
        rename_btn.connect_clicked(move |_| {
            let find_text = find_e.text().to_string();
            if find_text.is_empty() {
                w.announce("Enter text to find", AccessibleAnnouncementPriority::Medium);
                return;
            }
            let replace_text = replace_e.text().to_string();

            // Compute (old_path, new_path) pairs for entries that actually
            // change name. Skip identity renames.
            let mut pairs: Vec<(String, String)> = Vec::new();
            for (i, name) in names_rc.iter().enumerate() {
                let new_name = name.replace(&find_text, &replace_text);
                if new_name == *name {
                    continue;
                }
                let old_path = &file_paths[i];
                let new_path = std::path::Path::new(old_path)
                    .parent()
                    .map(|p| p.join(&new_name).to_string_lossy().to_string())
                    .unwrap_or_else(|| new_name.clone());
                pairs.push((old_path.clone(), new_path));
            }

            if pairs.is_empty() {
                w.announce(
                    "No matches to rename",
                    AccessibleAnnouncementPriority::Medium,
                );
                d.close();
                return;
            }

            // Two files given the same new name: the second would replace
            // the first.
            if let Some(name) = duplicate_target(&pairs) {
                w.announce(
                    &format!("Not renamed: more than one file would be called {name}"),
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }

            // If every old path lives on a recognised rclone mount, batch
            // through rclone-direct. Mixed-batch falls back to per-file GIO
            // (existing behaviour).
            let all_rclone = pairs
                .iter()
                .all(|(old, _)| wayfinder::rclone_ops::rclone_for_path(old).is_some())
                && wayfinder::rclone_ops::rclone_available();

            if all_rclone {
                let parent: gtk::Window = w.clone().upcast();
                let changed = vec![w.imp().model.current_path()];
                let moved: Vec<String> = pairs.iter().map(|(old, _)| old.clone()).collect();
                d.close();
                // rclone would overwrite a file that already has a new name;
                // GIO's rename refuses, and so do we. (Changing only the
                // case of a name is the same file.)
                let check: Vec<(String, String)> = pairs.clone();
                w.run_blocking(
                    &changed[0].clone(),
                    move || {
                        check.iter().find_map(|(old, new)| {
                            rename_clash(old, new).then(|| {
                                std::path::Path::new(new)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default()
                            })
                        })
                    },
                    move |w, existing| {
                        if let Some(name) = existing {
                            w.announce(
                                &format!("Not renamed: {name} already exists"),
                                AccessibleAnnouncementPriority::High,
                            );
                            return;
                        }
                        // The progress dialog announces the outcome.
                        let w_done = w.clone();
                        let on_complete: Option<Box<dyn FnOnce() + 'static>> =
                            Some(Box::new(move || {
                                w_done.forget_moved(&moved);
                                w_done.reload_after_change(&changed);
                            }));
                        wayfinder::rclone_ops::batch_rename_with_progress(
                            pairs,
                            &parent,
                            on_complete,
                        );
                    },
                );
                d.close();
                return;
            }

            d.close();
            let current = w.imp().model.current_path();
            let changed = vec![current.clone()];
            let moved: Vec<String> = pairs.iter().map(|(old, _)| old.clone()).collect();
            w.run_blocking(
                &current,
                move || {
                    let mut renamed = 0u32;
                    let mut errors = 0u32;
                    for (old_path, new_path) in &pairs {
                        let new_name = std::path::Path::new(new_path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| new_path.clone());
                        let gio_file = gio::File::for_path(old_path);
                        match wayfinder::file_ops::rename_file(&gio_file, &new_name) {
                            Ok(_) => renamed += 1,
                            Err(e) => {
                                log::error!("Batch rename error: {e}");
                                errors += 1;
                            }
                        }
                    }
                    (renamed, errors)
                },
                move |w, (renamed, errors)| {
                    let msg = if errors > 0 {
                        format!("Renamed {renamed} files, {errors} errors")
                    } else {
                        format!("Renamed {renamed} files")
                    };
                    w.announce(&msg, AccessibleAnnouncementPriority::Medium);

                    // Reload directory
                    w.forget_moved(&moved);
                    w.reload_after_change(&changed);
                },
            );
        });

        let key_ctrl = gtk::EventControllerKey::new();
        let d = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);

        let w = window.clone();
        dlg.connect_close_request(move |_| {
            w.restore_focus_to_selected();
            glib::Propagation::Proceed
        });

        dlg.present();
        find_entry.grab_focus();
    }

    pub fn handle_drop(&self, uri_str: &str) {
        if self.refuse_in_offline_view() {
            return;
        }
        let imp = self.imp();
        if imp.pasting.get() {
            self.announce(
                "A copy or paste is already in progress",
                AccessibleAnnouncementPriority::Medium,
            );
            return;
        }

        let paths = dropped_paths(uri_str);
        if paths.is_empty() {
            return;
        }

        // Set the guard before any per-path resolution work so two rapid
        // drops can't both pass the check.
        let guard = PastingGuard::new(self.clone());

        let dest_dir_str = imp.model.current_path();
        let use_rclone = (paths
            .iter()
            .any(|p| wayfinder::rclone_ops::rclone_for_path(p).is_some())
            || wayfinder::rclone_ops::rclone_for_path(&dest_dir_str).is_some())
            && wayfinder::rclone_ops::rclone_available();
        // A drop copies, with the same care as a paste: nothing into
        // itself, nothing overwritten without asking.
        let plan = PastePlan {
            files: paths.iter().map(gio::File::for_path).collect(),
            moved: landing_paths(&paths, &dest_dir_str),
            changed: vec![dest_dir_str.clone()],
            source_paths: paths,
            dest_dir: dest_dir_str,
            is_move: false,
        };
        self.dispatch_paste(plan, guard, use_rclone);
    }

    pub fn create_new_folder(&self) {
        if self.refuse_in_offline_view() {
            return;
        }
        let window = self.clone();

        let entry = gtk::Entry::builder()
            .text("New Folder")
            .hexpand(true)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Folder name")]);
        entry.select_region(0, -1);

        let dlg = gtk::Window::builder()
            .title("New Folder")
            .modal(true)
            .transient_for(&window)
            .default_width(400)
            .build();

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let label = gtk::Label::new(Some("Create new folder"));
        vbox.append(&label);
        vbox.append(&entry);

        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        button_box.set_halign(gtk::Align::End);

        let cancel_btn = gtk::Button::with_label("Cancel");
        let create_btn = gtk::Button::with_label("Create");
        create_btn.add_css_class("suggested-action");
        button_box.append(&cancel_btn);
        button_box.append(&create_btn);
        vbox.append(&button_box);

        dlg.set_child(Some(&vbox));

        let d = dlg.clone();
        cancel_btn.connect_clicked(move |_| {
            d.close();
        });

        let d = dlg.clone();
        let entry_clone = entry.clone();
        let w = window.clone();
        let do_create = move || {
            let name = entry_clone.text().to_string();
            if name.is_empty() {
                d.close();
                return;
            }
            d.close();
            let current = w.imp().model.current_path();
            let parent = current.clone();
            let changed = vec![current.clone()];
            w.run_blocking(
                &current,
                move || {
                    wayfinder::file_ops::create_folder(&gio::File::for_path(&parent), &name)
                        .map(|_| name)
                        .map_err(|e| e.to_string())
                },
                move |w, result| match result {
                    Ok(name) => {
                        w.announce(
                            &format!("Created folder {name}"),
                            AccessibleAnnouncementPriority::Medium,
                        );
                        // Reload directory as fallback in case file monitor doesn't catch the new folder
                        w.reload_after_change(&changed);
                    }
                    Err(e) => {
                        w.announce(
                            &format!("Failed to create folder: {e}"),
                            AccessibleAnnouncementPriority::High,
                        );
                    }
                },
            );
        };

        let do_create_clone = do_create.clone();
        create_btn.connect_clicked(move |_| {
            do_create_clone();
        });

        entry.connect_activate(move |_| {
            do_create();
        });

        let key_ctrl = gtk::EventControllerKey::new();
        let d = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);

        let w = window.clone();
        dlg.connect_close_request(move |_| {
            w.restore_focus_to_selected();
            glib::Propagation::Proceed
        });

        dlg.present();
        entry.grab_focus();
    }

    pub fn undo_trash(&self) {
        let paths = self.imp().last_trashed.borrow().clone();
        if paths.is_empty() {
            self.announce("Nothing to undo", AccessibleAnnouncementPriority::Medium);
            return;
        }

        let trash = gio::File::for_uri("trash:///");
        let mut restored = 0;
        for path in &paths {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // Find the item in trash by original path — the most recently
            // trashed, if an older copy from the same place is there too.
            let mut newest: Option<(String, std::path::PathBuf)> = None;
            if let Ok(enumerator) = trash.enumerate_children(
                "standard::name,trash::orig-path,trash::deletion-date",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            ) {
                while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
                    let orig = info
                        .attribute_byte_string("trash::orig-path")
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    if orig != *path {
                        continue;
                    }
                    // ISO 8601 local time: later sorts after.
                    let date = info
                        .attribute_string("trash::deletion-date")
                        .map(|d| d.to_string())
                        .unwrap_or_default();
                    if newest.as_ref().is_none_or(|(d, _)| date > *d) {
                        newest = Some((date, info.name()));
                    }
                }
            }
            if let Some((_, trashed_name)) = newest {
                match wayfinder::file_ops::restore_from_trash(&trash.child(trashed_name)) {
                    Ok(_) => restored += 1,
                    Err(e) => {
                        log::error!("Failed to restore {name}: {e}");
                    }
                }
            }
        }

        self.imp().last_trashed.borrow_mut().clear();

        if restored > 0 {
            // Reload directory to show restored files, and drop the cached
            // listings of the folders they went back to.
            let mut folders: Vec<String> = Vec::new();
            for parent in paths.iter().filter_map(|p| parent_dir(p)) {
                if !folders.contains(&parent) {
                    folders.push(parent);
                }
            }
            self.reload_after_change(&folders);

            if restored == 1 {
                self.announce(
                    &format!(
                        "Restored {}",
                        std::path::Path::new(&paths[0])
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ),
                    AccessibleAnnouncementPriority::Medium,
                );
            } else {
                self.announce(
                    &format!("Restored {restored} files"),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
        } else {
            self.announce(
                "Could not restore files",
                AccessibleAnnouncementPriority::High,
            );
        }
    }

    pub fn open_terminal_here(&self) {
        let path = self.imp().model.current_path();
        // Try common terminals in order
        let terminals: &[(&str, &[&str])] = &[
            ("foot", &["--working-directory"]),
            ("alacritty", &["--working-directory"]),
            ("gnome-terminal", &["--working-directory"]),
            ("konsole", &["--workdir"]),
        ];

        for (cmd, args) in terminals {
            if std::process::Command::new("which")
                .arg(cmd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                let mut full_args: Vec<&str> = args.to_vec();
                full_args.push(&path);
                let _ = std::process::Command::new(cmd).args(&full_args).spawn();
                self.announce(
                    &format!("Opened terminal in {path}"),
                    AccessibleAnnouncementPriority::Medium,
                );
                return;
            }
        }
        self.announce(
            "No terminal emulator found",
            AccessibleAnnouncementPriority::High,
        );
    }

    pub fn show_shortcuts(&self) {
        let dlg = gtk::Window::builder()
            .title("Keyboard Shortcuts")
            .modal(true)
            .transient_for(self)
            .default_width(500)
            .default_height(600)
            .build();
        dlg.update_property(&[gtk::accessible::Property::Label("Keyboard shortcuts")]);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("rich-list");
        list.update_property(&[gtk::accessible::Property::Label("Shortcuts list")]);

        let sections: &[(&str, &[(&str, &str)])] = &[
            (
                "Navigation",
                &[
                    ("Alt+Left", "Go back"),
                    ("Alt+Right", "Go forward"),
                    ("Alt+Up", "Go to parent folder"),
                    ("Ctrl+L", "Go to location"),
                    ("Ctrl+Shift+H", "Home"),
                    ("Ctrl+Shift+O", "Documents"),
                    ("Ctrl+Shift+K", "Desktop"),
                    ("Ctrl+Shift+L", "Downloads"),
                    ("Ctrl+Shift+R", "File System"),
                    ("Enter", "Open file or folder"),
                    ("Backspace", "Go back"),
                ],
            ),
            (
                "File Operations",
                &[
                    ("Ctrl+C", "Copy"),
                    ("Ctrl+X", "Cut"),
                    ("Ctrl+V", "Paste"),
                    ("Ctrl+Shift+C", "Copy (this window only)"),
                    ("Ctrl+Shift+X", "Cut (this window only)"),
                    ("Ctrl+Shift+V", "Paste (this window only)"),
                    ("Ctrl+A", "Select all"),
                    ("Space", "Toggle selection"),
                    ("Shift+Space", "Range selection"),
                    ("Escape", "Clear selection"),
                    ("F2", "Rename"),
                    ("Ctrl+Shift+F2", "Batch rename"),
                    ("Delete", "Move to Bin"),
                    ("Shift+Delete", "Delete permanently"),
                    ("Ctrl+Shift+N", "New folder"),
                    ("Ctrl+D", "Bookmark current folder"),
                    ("Ctrl+Z", "Undo trash"),
                ],
            ),
            (
                "View",
                &[
                    ("Ctrl+1", "Grid view"),
                    ("Ctrl+2", "List view"),
                    ("Ctrl+H", "Toggle hidden files"),
                    ("F3", "Toggle sidebar"),
                    ("F4", "Toggle breadcrumb bar"),
                    ("Ctrl+F", "Search files"),
                    ("Ctrl+I", "Properties"),
                    ("Ctrl++", "Zoom in"),
                    ("Ctrl+-", "Zoom out"),
                    ("Ctrl+0", "Reset zoom"),
                ],
            ),
            (
                "General",
                &[
                    ("Ctrl+N", "New window"),
                    ("Ctrl+`", "Open terminal here"),
                    ("Menu or Shift+F10", "Context menu"),
                    ("Tab", "Path completion (in location bar)"),
                    ("Type letters", "Jump to matching file"),
                    ("Ctrl+?", "This shortcuts window"),
                ],
            ),
        ];

        for (section_name, shortcuts) in sections {
            // Section header
            let header = gtk::Label::builder()
                .label(*section_name)
                .xalign(0.0)
                .css_classes(["heading"])
                .margin_top(12)
                .margin_bottom(4)
                .margin_start(8)
                .build();
            let header_row = gtk::ListBoxRow::new();
            header_row.set_child(Some(&header));
            header_row.set_selectable(false);
            header_row.set_activatable(false);
            list.append(&header_row);

            for (key, description) in *shortcuts {
                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                hbox.set_margin_top(4);
                hbox.set_margin_bottom(4);
                hbox.set_margin_start(16);
                hbox.set_margin_end(8);

                let desc_label = gtk::Label::builder()
                    .label(*description)
                    .xalign(0.0)
                    .hexpand(true)
                    .build();

                let key_label = gtk::Label::builder()
                    .label(*key)
                    .xalign(1.0)
                    .css_classes(["dim-label"])
                    .build();

                hbox.append(&desc_label);
                hbox.append(&key_label);

                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&hbox));
                row.set_selectable(false);
                row.set_activatable(false);
                row.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{description}: {key}"
                ))]);

                list.append(&row);
            }
        }

        scrolled.set_child(Some(&list));
        vbox.append(&scrolled);

        let close_btn = gtk::Button::with_label("Close");
        close_btn.set_halign(gtk::Align::End);
        close_btn.set_margin_top(12);
        let d = dlg.clone();
        close_btn.connect_clicked(move |_| d.close());
        vbox.append(&close_btn);

        dlg.set_child(Some(&vbox));

        let key_ctrl = gtk::EventControllerKey::new();
        let d = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);

        let w = self.clone();
        dlg.connect_close_request(move |_| {
            w.restore_focus_to_selected();
            glib::Propagation::Proceed
        });

        dlg.present();
    }

    /// Update the breadcrumb path bar with clickable segments.
    fn update_breadcrumb(&self, path: &str) {
        let imp = self.imp();
        let breadcrumb = &imp.breadcrumb_box;

        // Remove existing buttons
        while let Some(child) = breadcrumb.first_child() {
            breadcrumb.remove(&child);
        }

        // Special URIs
        if path.starts_with("trash:") || path.starts_with("recent:") {
            let label = if path.starts_with("recent:") {
                "Recent Files"
            } else {
                "Bin"
            };
            let btn = gtk::Button::with_label(label);
            btn.add_css_class("flat");
            btn.set_sensitive(false);
            btn.update_property(&[gtk::accessible::Property::Label(label)]);
            breadcrumb.append(&btn);
            return;
        }

        // Build path segments
        let components: Vec<&str> = path.split('/').collect();
        let mut accumulated = String::new();

        for (i, component) in components.iter().enumerate() {
            if i == 0 {
                // Root "/"
                accumulated.push('/');
                let btn = gtk::Button::with_label("/");
                btn.add_css_class("flat");
                btn.update_property(&[gtk::accessible::Property::Label("Root directory")]);
                let w = self.clone();
                btn.connect_clicked(move |_| {
                    w.navigate_to_path("/");
                });
                breadcrumb.append(&btn);
                continue;
            }

            if component.is_empty() {
                continue;
            }

            accumulated.push_str(component);

            // Separator
            let sep = gtk::Label::new(Some("/"));
            sep.add_css_class("dim-label");
            breadcrumb.append(&sep);

            let btn = gtk::Button::with_label(component);
            btn.add_css_class("flat");
            let target = accumulated.clone();
            let display = *component;
            btn.update_property(&[gtk::accessible::Property::Label(&format!(
                "Go to {display}"
            ))]);

            // Last segment is not clickable (current directory)
            if i == components.len() - 1 {
                btn.set_sensitive(false);
            } else {
                let w = self.clone();
                btn.connect_clicked(move |_| {
                    w.navigate_to_path(&target);
                });
            }

            breadcrumb.append(&btn);
            accumulated.push('/');
        }

        // Scroll to the end to show the current directory
        let scroll = imp.breadcrumb_scroll.clone();
        glib::idle_add_local_once(move || {
            let adj = scroll.hadjustment();
            adj.set_value(adj.upper());
        });
    }

    /// Adjust zoom level by `delta` percent (e.g. +10 or -10).
    /// Passing 0 just re-applies the current level (used by reset).
    pub fn apply_zoom(&self, delta: i32) {
        let imp = self.imp();
        let new_level = (imp.zoom_level.get() + delta).clamp(50, 300);
        imp.zoom_level.set(new_level);

        let pt = new_level as f64 / 100.0 * 10.0;
        imp.zoom_css
            .load_from_string(&format!("* {{ font-size: {pt}pt; }}"));

        wayfinder::state::save_zoom_level(new_level);

        self.announce(
            &format!("Zoom {new_level}%"),
            AccessibleAnnouncementPriority::Medium,
        );
    }
}

/// Check if a file has the execute bit set and is a regular file.
fn is_executable_file(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    meta.permissions().mode() & 0o111 != 0
}

/// Check if an executable is a text-based script (has a shebang or is
/// recognised text content) vs a compiled binary.
fn is_text_executable(path: &str) -> bool {
    // Read first 4 bytes to detect ELF magic (binary) vs shebang (#!)
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut buf = [0u8; 4];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    if n < 2 {
        return false;
    }
    // ELF magic bytes: 0x7F 'E' 'L' 'F' → binary
    if n >= 4 && &buf[..4] == b"\x7fELF" {
        return false;
    }
    // Shebang: starts with #!
    if &buf[..2] == b"#!" {
        return true;
    }
    // Otherwise assume binary (could be a script without shebang, but rare)
    false
}

/// Shell-quote a path for safe inclusion in a sh -c command.
fn shell_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Check whether `path` resides on a FUSE filesystem according to /proc/mounts.
fn is_fuse_mount(path: &str) -> bool {
    wayfinder::rclone_ops::is_fuse_path(path)
}

/// What a paste copies or moves, and where.
struct PastePlan {
    files: Vec<gio::File>,
    source_paths: Vec<String>,
    dest_dir: String,
    is_move: bool,
    /// Folders to reload (see `reload_after_change`).
    changed: Vec<String>,
    /// Locations whose cached listings are stale (see `forget_moved`).
    moved: Vec<String>,
}

/// Work put off until something is answered.
type Deferred = std::rc::Rc<std::cell::RefCell<Option<Box<dyn FnOnce()>>>>;

/// Where items are being removed from, to select near them afterwards.
#[derive(Clone)]
struct RemovedFrom {
    folder: String,
    /// The navigation then; a new one means the user has been elsewhere.
    generation: u64,
    /// The position selected then.
    old_pos: u32,
    /// The items selected for removal.
    removed: Vec<String>,
}

impl RemovedFrom {
    fn here(window: &WayfinderWindow) -> Self {
        let imp = window.imp();
        Self {
            folder: imp.model.current_path(),
            generation: imp.model.generation(),
            old_pos: imp.selection.selected(),
            removed: window
                .get_selected_files()
                .iter()
                .map(|f| f.path())
                .collect(),
        }
    }
}

/// Spawn a threaded delete with a non-modal progress dialog. Used for
/// FUSE mounts where each per-file delete may take seconds.
fn run_threaded_delete(
    window: &WayfinderWindow,
    file_paths: Vec<(String, String)>,
    from: RemovedFrom,
) {
    let parent: gtk::Window = window.clone().upcast();
    let paths: Vec<String> = file_paths.iter().map(|(_, p)| p.clone()).collect();
    let names: Vec<String> = file_paths.iter().map(|(n, _)| n.clone()).collect();
    let total = paths.len();
    let win = window.clone();
    let on_complete: Option<Box<dyn FnOnce(usize, usize) + 'static>> =
        Some(Box::new(move |success, failed| {
            // Reload so the UI reflects the deletions, then restore focus
            // near the previous position — unless nothing was deleted
            // (cancelled, or every delete failed).
            if success > 0 {
                win.reload_after_removal(from, |_, _| {});
            } else {
                // A folder may be partly gone: show what's there now, but
                // leave the selection where the user put it.
                win.forget_moved(&from.removed);
                win.reload_after_change(std::slice::from_ref(&from.folder));
            }

            // Announce outcome
            if failed == 0 && success > 0 {
                let msg = if success == 1 {
                    format!("Deleted {}", names[0])
                } else {
                    format!("Deleted {success} of {total} items")
                };
                win.announce(&msg, AccessibleAnnouncementPriority::Medium);
            } else if failed > 0 && success == 0 {
                win.announce(
                    &format!("Failed to delete {failed} item(s)"),
                    AccessibleAnnouncementPriority::High,
                );
            } else if failed > 0 {
                win.announce(
                    &format!("Deleted {success}, failed to delete {failed}"),
                    AccessibleAnnouncementPriority::High,
                );
            } else {
                win.announce("Deletion cancelled", AccessibleAnnouncementPriority::Medium);
            }
        }));
    wayfinder::file_ops::delete_with_progress(paths, &parent, on_complete);
}

/// Run a permanent delete via direct `rclone deletefile`/`purge` calls,
/// bypassing the FUSE layer. The rclone progress dialog handles its own
/// success/failure announcements.
fn run_rclone_direct_delete(
    window: &WayfinderWindow,
    rclone_items: Vec<(String, bool)>,
    from: RemovedFrom,
) {
    let parent: gtk::Window = window.clone().upcast();
    let win = window.clone();
    let on_complete: Option<Box<dyn FnOnce(bool) + 'static>> = Some(Box::new(move |succeeded| {
        // The progress dialog already announced success/failure — don't
        // override it with a misleading "Deleted N" if rclone errored or
        // the user cancelled.
        if succeeded {
            win.reload_after_removal(from, |_, _| {});
        } else {
            // Some items may be gone: show what's there now, but leave the
            // selection where the user put it.
            win.forget_moved(&from.removed);
            win.reload_after_change(std::slice::from_ref(&from.folder));
        }
    }));
    wayfinder::rclone_ops::delete_with_progress(rclone_items, &parent, on_complete);
}

/// The local paths in dropped text: a `text/uri-list` (one URI per line,
/// `#` lines are comments) or a plain path. URIs are decoded, so
/// `file:///home/u/My%20Report.pdf` gives `/home/u/My Report.pdf`.
fn dropped_paths(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            if line.starts_with("file://") {
                gio::File::for_uri(line)
                    .path()
                    .map(|p| p.to_string_lossy().to_string())
            } else if line.starts_with('/') {
                Some(line.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Where `sources` land when copied or moved into `dest_dir`.
fn landing_paths(sources: &[String], dest_dir: &str) -> Vec<String> {
    sources
        .iter()
        .filter_map(|p| std::path::Path::new(p).file_name())
        .map(|name| {
            std::path::Path::new(dest_dir)
                .join(name)
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

/// Whether renaming `old` to `new` would replace another file. A name that
/// differs only in case is the same file on a case-insensitive remote —
/// but on a case-sensitive one it's another file, so compare identities.
fn rename_clash(old: &str, new: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(new_meta) = std::fs::symlink_metadata(new) else {
        return false;
    };
    match std::fs::symlink_metadata(old) {
        Ok(old_meta) => (old_meta.dev(), old_meta.ino()) != (new_meta.dev(), new_meta.ino()),
        Err(_) => true,
    }
}

/// The new name given to more than one file, if any, in rename `pairs`.
fn duplicate_target(pairs: &[(String, String)]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    pairs.iter().find_map(|(_, new)| {
        (!seen.insert(new.as_str())).then(|| {
            std::path::Path::new(new)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        })
    })
}

/// The folder holding `path`.
fn parent_dir(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
}

/// Where to put the selection after removing the item at `old_pos`, with
/// `n_items` left: the item above it, or the new last item. `None` if
/// nothing is left.
fn position_near_removed(old_pos: u32, n_items: u32) -> Option<u32> {
    if n_items == 0 {
        None
    } else if old_pos == 0 {
        Some(0)
    } else {
        Some((old_pos - 1).min(n_items - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dropped_paths, duplicate_target, landing_paths, position_near_removed, rename_clash,
    };

    #[test]
    fn renaming_onto_another_file_clashes() {
        let d = std::env::temp_dir().join(format!("wayfinder-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("Report.txt"), d.join("report.txt"));
        std::fs::write(&a, b"a").unwrap();
        let (a, b) = (
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        );
        // Nothing there yet.
        assert!(!rename_clash(&a, &b));
        // A different file with the name differing only in case (a
        // case-sensitive filesystem): a clash.
        std::fs::write(&b, b"b").unwrap();
        assert!(rename_clash(&a, &b));
        // The same file.
        assert!(!rename_clash(&a, &a));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn batch_rename_to_one_name_is_caught() {
        let pairs = vec![
            ("/d/doc v1.txt".to_string(), "/d/doc .txt".to_string()),
            ("/d/doc .txt".to_string(), "/d/doc .txt".to_string()),
            ("/d/other.txt".to_string(), "/d/x.txt".to_string()),
        ];
        assert_eq!(duplicate_target(&pairs), Some("doc .txt".to_string()));
        assert_eq!(duplicate_target(&pairs[2..]), None);
    }

    #[test]
    fn pasted_items_land_in_the_destination_under_their_names() {
        let sources = vec!["/a/Photos".to_string(), "/b/notes.txt".to_string()];
        assert_eq!(
            landing_paths(&sources, "/mnt/cloud/Docs"),
            vec!["/mnt/cloud/Docs/Photos", "/mnt/cloud/Docs/notes.txt"]
        );
    }

    #[test]
    fn dropped_uris_are_decoded() {
        assert_eq!(
            dropped_paths("file:///home/u/My%20Report.pdf"),
            vec!["/home/u/My Report.pdf"]
        );
    }

    #[test]
    fn dropped_uri_list_gives_every_file() {
        let list = "# from a file manager\r\nfile:///a/one.txt\r\nfile:///a/two%23.txt\r\n";
        assert_eq!(dropped_paths(list), vec!["/a/one.txt", "/a/two#.txt"]);
    }

    #[test]
    fn dropped_plain_path_is_kept() {
        assert_eq!(dropped_paths("/a/b c.txt"), vec!["/a/b c.txt"]);
        assert!(dropped_paths("https://example.com/x").is_empty());
    }

    #[test]
    fn selection_moves_to_the_item_above() {
        assert_eq!(position_near_removed(5, 10), Some(4));
    }

    #[test]
    fn removing_the_first_item_keeps_the_top() {
        assert_eq!(position_near_removed(0, 10), Some(0));
    }

    #[test]
    fn removing_from_the_end_selects_the_new_last_item() {
        // Three removed from the end of ten: positions 7..9 gone.
        assert_eq!(position_near_removed(9, 7), Some(6));
    }

    #[test]
    fn nothing_left_selects_nothing() {
        assert_eq!(position_near_removed(3, 0), None);
    }
}
