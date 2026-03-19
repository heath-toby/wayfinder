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

glib::wrapper! {
    pub struct WayfinderWindow(ObjectSubclass<imp::WayfinderWindowInner>)
        @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl WayfinderWindow {
    pub fn new(app: &Application) -> Self {
        glib::Object::builder()
            .property("application", app)
            .build()
    }

    /// Navigate to a path, updating history. If the path doesn't exist,
    /// walk up the tree until we find a valid parent directory.
    pub fn navigate_to_path(&self, path: &str) {
        let mut path_buf = PathBuf::from(path);
        while !path_buf.is_dir() {
            match path_buf.parent() {
                Some(parent) => path_buf = parent.to_path_buf(),
                None => {
                    path_buf = PathBuf::from("/");
                    break;
                }
            }
        }

        let resolved = path_buf.to_string_lossy().to_string();
        self.imp().nav.borrow_mut().navigate_to(path_buf);
        self.load_directory(&resolved);
    }

    /// Load a directory without modifying history (used by back/forward)
    pub fn load_directory(&self, path: &str) {
        let imp = self.imp();

        // Clear search when navigating
        if imp.model.search.is_active() {
            imp.model.search.clear();
            imp.search_entry.set_text("");
            imp.search_bar.set_search_mode(false);
        }

        match imp.model.load_directory(path) {
            Ok(_count) => {
                imp.location_entry.set_text(path);

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

                // Reset column, selection state, type-ahead, and focus the first item
                imp.current_column.set(0);
                imp.file_selection.borrow_mut().clear();
                imp.type_ahead_buffer.borrow_mut().clear();
                imp.selection.set_selected(0);

                // Announce before focus for empty folders (focus change
                // would otherwise override the announcement)
                if count == 0 {
                    self.announce(
                        &format!("Opened {}, folder is empty", dir_name),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }

                self.focus_current_view();

                if count > 0 {
                    self.announce(
                        &format!("Opened {}, {} items", dir_name, count),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }
            }
            Err(e) => {
                log::error!("Failed to load directory {}: {}", path, e);
                self.announce(
                    &format!("Error: could not open {}", path),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
    }

    pub fn load_special_uri(&self, uri: &str) {
        let imp = self.imp();

        if imp.model.search.is_active() {
            imp.model.search.clear();
            imp.search_entry.set_text("");
            imp.search_bar.set_search_mode(false);
        }

        match imp.model.load_uri(uri) {
            Ok(_count) => {
                imp.location_entry.set_text(uri);
                imp.back_button
                    .set_sensitive(imp.nav.borrow().can_go_back());
                imp.forward_button
                    .set_sensitive(imp.nav.borrow().can_go_forward());
                imp.up_button.set_sensitive(false);

                self.update_status();

                let count = imp.model.item_count();

                imp.current_column.set(0);
                imp.selection.set_selected(0);

                if count == 0 {
                    self.announce("Opened Bin, Bin is empty", AccessibleAnnouncementPriority::Medium);
                }

                self.focus_current_view();

                if count > 0 {
                    self.announce(&format!("Opened Bin, {} items", count), AccessibleAnnouncementPriority::Medium);
                }
            }
            Err(e) => {
                log::error!("Failed to load {}: {}", uri, e);
                self.announce(
                    &format!("Error: could not open Bin: {}", e),
                    AccessibleAnnouncementPriority::High,
                );
            }
        }
    }

    pub fn update_status(&self) {
        let imp = self.imp();
        let count = imp.model.item_count();
        let sel_count = imp.file_selection.borrow().count();
        let mut parts = vec![format!("{} items", count)];
        if sel_count > 0 {
            parts.push(format!("{} selected", sel_count));
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
                imp.list_view.column_view().set_model(gtk::SelectionModel::NONE);
                imp.grid_view.set_model(&imp.selection);
                imp.view_stack.set_visible_child_name("grid");
                wayfinder::state::save_view_mode("grid");
                self.announce(
                    "Switched to grid view",
                    AccessibleAnnouncementPriority::Medium,
                );
            }
            ViewMode::List => {
                imp.grid_view.grid_view().set_model(gtk::SelectionModel::NONE);
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

        // Check for per-file app association
        if let Some(desktop_id) = wayfinder::state::load_file_app(&path) {
            let all_apps = gio::AppInfo::all();
            if let Some(app) = all_apps.iter().find(|a| {
                a.id().map(|id| id.to_string()) == Some(desktop_id.clone())
            }) {
                let gio_file = gio::File::for_path(&path);
                let ctx = WidgetExt::display(self).app_launch_context();
                if let Err(e) = app.launch(&[gio_file], Some(&ctx)) {
                    log::error!("Failed to open with preferred app: {}", e);
                    // Fall through to default
                } else {
                    return;
                }
            }
        }

        // Fall back to MIME type default
        let gio_file = gio::File::for_path(&path);
        let uri = gio_file.uri();
        let ctx = WidgetExt::display(self).app_launch_context();
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, Some(&ctx)) {
            log::error!("Failed to open {}: {}", path, e);
            self.announce(
                &format!("Failed to open {}", file.name()),
                AccessibleAnnouncementPriority::High,
            );
        }
    }

    pub fn active_view_widget(&self) -> gtk::Widget {
        let imp = self.imp();
        match imp.current_view.get() {
            ViewMode::Grid => imp.grid_view.grid_view().clone().upcast(),
            ViewMode::List => imp.list_view.column_view().clone().upcast(),
        }
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

        let gio_file = gio::File::for_uri(&format!("trash:///{}", file.name()));
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
                        match wayfinder::file_ops::empty_trash() {
                            Ok(count) => {
                                window.announce(
                                    &format!("Bin emptied, {} items deleted", count),
                                    AccessibleAnnouncementPriority::Medium,
                                );
                                // Reload if we're viewing trash
                                if window.is_in_trash() {
                                    window.load_special_uri("trash:///");
                                }
                            }
                            Err(e) => {
                                window.announce(
                                    &format!("Failed to empty bin: {}", e),
                                    AccessibleAnnouncementPriority::High,
                                );
                            }
                        }
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
        let gio_files: Vec<_> = files.iter().map(|f| gio::File::for_path(f.path())).collect();
        let count = gio_files.len();
        wayfinder::clipboard::global_set(ClipboardState::new(ClipboardOperation::Copy, gio_files));
        if count == 1 {
            self.announce(
                &format!("Copied {}", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Copied {} files", count),
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
        let gio_files: Vec<_> = files.iter().map(|f| gio::File::for_path(f.path())).collect();
        let count = gio_files.len();
        wayfinder::clipboard::global_set(ClipboardState::new(ClipboardOperation::Cut, gio_files));
        if count == 1 {
            self.announce(
                &format!("Cut {}", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Cut {} files", count),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Paste from the global (cross-window) clipboard.
    pub fn paste(&self) {
        self.paste_from(wayfinder::clipboard::global_get(), true);
    }

    /// Copy selected files to the window-local clipboard.
    pub fn copy_selected_local(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        let gio_files: Vec<_> = files.iter().map(|f| gio::File::for_path(f.path())).collect();
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
                &format!("Copied {} files (this window)", count),
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
        let gio_files: Vec<_> = files.iter().map(|f| gio::File::for_path(f.path())).collect();
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
                &format!("Cut {} files (this window)", count),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    /// Paste from the window-local clipboard.
    pub fn paste_local(&self) {
        self.paste_from(self.imp().clipboard.borrow().clone(), false);
    }

    fn paste_from(&self, clipboard: Option<ClipboardState>, is_global: bool) {
        let imp = self.imp();
        if let Some(state) = clipboard {
            let dest_dir = gio::File::for_path(imp.model.current_path());
            let parent_window: gtk::Window = self.clone().upcast();

            for source in &state.files {
                let w = self.clone();
                let reload: Option<Box<dyn FnOnce() + 'static>> = Some(Box::new(move || {
                    let path = w.imp().model.current_path();
                    let _ = w.imp().model.load_directory(&path);
                    w.update_status();
                }));
                match state.operation {
                    ClipboardOperation::Copy => {
                        wayfinder::file_ops::copy_with_progress(source, &dest_dir, &parent_window, reload);
                    }
                    ClipboardOperation::Cut => {
                        wayfinder::file_ops::move_with_progress(source, &dest_dir, &parent_window, reload);
                    }
                }
            }

            // Clear clipboard after cut
            if state.operation == ClipboardOperation::Cut {
                if is_global {
                    wayfinder::clipboard::global_clear();
                } else {
                    *imp.clipboard.borrow_mut() = None;
                }
            }
        } else {
            self.announce("Nothing to paste", AccessibleAnnouncementPriority::Medium);
        }
    }

    pub fn trash_selected(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }
        // Remember position before deletion
        let imp = self.imp();
        let old_pos = imp.selection.selected();

        // Save paths for undo
        let paths: Vec<String> = files.iter().map(|f| f.path()).collect();

        let mut success = 0;
        let mut failed = 0;
        let mut last_error = String::new();
        for file in &files {
            let gio_file = gio::File::for_path(file.path());
            match wayfinder::file_ops::trash_file(&gio_file) {
                Ok(()) => success += 1,
                Err(e) => {
                    failed += 1;
                    last_error = format!("{}: {}", file.name(), e);
                }
            }
        }

        // Store trashed paths for undo (only successfully trashed ones)
        if success > 0 {
            *imp.last_trashed.borrow_mut() = paths;
        }

        // Announce failures FIRST (before focus changes trigger Orca)
        if failed > 0 {
            if failed == 1 {
                self.announce(
                    &format!("Could not move to Bin: {}", last_error),
                    AccessibleAnnouncementPriority::High,
                );
            } else {
                self.announce(
                    &format!("{} files could not be moved to Bin", failed),
                    AccessibleAnnouncementPriority::High,
                );
            }
            if success == 0 {
                return;
            }
        }

        imp.file_selection.borrow_mut().clear();
        self.update_status();

        // Focus the item above the deleted one (or the new last item)
        let n_items = imp.selection.n_items();
        if n_items > 0 {
            let new_pos = if old_pos > 0 && old_pos >= n_items {
                n_items - 1
            } else if old_pos > 0 {
                old_pos - 1
            } else {
                0
            };
            imp.selection.set_selected(new_pos);
            self.restore_focus_to_selected();
        }

        if failed == 0 {
            let now_empty = imp.selection.n_items() == 0;
            let msg = if success == 1 {
                if now_empty {
                    format!("Moved {} to Bin, folder is now empty", files[0].name())
                } else {
                    format!("Moved {} to Bin", files[0].name())
                }
            } else if now_empty {
                format!("Moved {} files to Bin, folder is now empty", success)
            } else {
                format!("Moved {} files to Bin", success)
            };

            self.announce(&msg, AccessibleAnnouncementPriority::Medium);
        }
    }

    pub fn delete_selected(&self) {
        let files = self.get_selected_files();
        if files.is_empty() {
            return;
        }

        let window = self.clone();
        let old_pos = self.imp().selection.selected();
        let count = files.len();

        let message = if count == 1 {
            format!("Permanently delete {}?", files[0].name())
        } else {
            format!("Permanently delete {} items?", count)
        };

        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .detail("This cannot be undone.")
            .buttons(["Cancel", "Delete permanently"])
            .cancel_button(0)
            .default_button(0)
            .build();

        // Capture paths as strings before the async callback
        let file_paths: Vec<(String, String)> = files
            .iter()
            .map(|f| (f.name(), f.path()))
            .collect();

        dialog.choose(
            Some(&window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(choice) = result {
                    if choice == 1 {
                        let mut success = 0;
                        let mut failed = 0;
                        let mut last_error = String::new();

                        for (name, path) in &file_paths {
                            let gio_file = gio::File::for_path(path);
                            match wayfinder::file_ops::delete_file_recursive(&gio_file) {
                                Ok(()) => success += 1,
                                Err(e) => {
                                    failed += 1;
                                    last_error = format!("{}: {}", name, e);
                                }
                            }
                        }

                        // Announce failures first
                        if failed > 0 {
                            if failed == 1 {
                                window.announce(
                                    &format!("Could not delete: {}", last_error),
                                    AccessibleAnnouncementPriority::High,
                                );
                            } else {
                                window.announce(
                                    &format!("{} files could not be deleted", failed),
                                    AccessibleAnnouncementPriority::High,
                                );
                            }
                        }

                        if success > 0 {
                            window.imp().file_selection.borrow_mut().clear();
                            window.update_status();

                            let n_items = window.imp().selection.n_items();
                            if n_items > 0 {
                                let new_pos = if old_pos > 0 && old_pos >= n_items {
                                    n_items - 1
                                } else if old_pos > 0 {
                                    old_pos - 1
                                } else {
                                    0
                                };
                                window.imp().selection.set_selected(new_pos);
                                window.restore_focus_to_selected();
                            }

                            if failed == 0 {
                                let now_empty = n_items == 0;
                                let msg = if success == 1 {
                                    if now_empty {
                                        format!("Deleted {}, folder is now empty", file_paths[0].0)
                                    } else {
                                        format!("Deleted {}", file_paths[0].0)
                                    }
                                } else if now_empty {
                                    format!("Deleted {} files, folder is now empty", success)
                                } else {
                                    format!("Deleted {} files", success)
                                };

                                window.announce(&msg, AccessibleAnnouncementPriority::Medium);
                            }
                        }
                    }
                }
            },
        );
    }

    pub fn rename_selected(&self) {
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
            let gio_file = gio::File::for_path(&file_path);
            match wayfinder::file_ops::rename_file(&gio_file, &new_name) {
                Ok(_) => {
                    w.announce(
                        &format!("Renamed to {}", new_name),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    // Reload directory as fallback in case file monitor doesn't catch the rename
                    let path = w.imp().model.current_path();
                    let _ = w.imp().model.load_directory(&path);
                    w.update_status();
                }
                Err(e) => {
                    w.announce(
                        &format!("Rename failed: {}", e),
                        AccessibleAnnouncementPriority::High,
                    );
                }
            }
            d.close();
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

    pub fn handle_drop(&self, uri_str: &str) {
        let path = if let Some(p) = uri_str.strip_prefix("file://") {
            p.to_string()
        } else {
            uri_str.to_string()
        };

        let source = gio::File::for_path(&path);
        let dest_dir = gio::File::for_path(self.imp().model.current_path());
        let parent_window: gtk::Window = self.clone().upcast();
        let w = self.clone();
        let reload: Option<Box<dyn FnOnce() + 'static>> = Some(Box::new(move || {
            let current = w.imp().model.current_path();
            let _ = w.imp().model.load_directory(&current);
            w.update_status();
        }));
        wayfinder::file_ops::copy_with_progress(&source, &dest_dir, &parent_window, reload);
    }

    pub fn create_new_folder(&self) {
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
            let parent = gio::File::for_path(w.imp().model.current_path());
            match wayfinder::file_ops::create_folder(&parent, &name) {
                Ok(_) => {
                    w.announce(
                        &format!("Created folder {}", name),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    // Reload directory as fallback in case file monitor doesn't catch the new folder
                    let path = w.imp().model.current_path();
                    let _ = w.imp().model.load_directory(&path);
                    w.update_status();
                }
                Err(e) => {
                    w.announce(
                        &format!("Failed to create folder: {}", e),
                        AccessibleAnnouncementPriority::High,
                    );
                }
            }
            d.close();
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
            // Find the item in trash by original path
            if let Ok(enumerator) = trash.enumerate_children(
                "standard::name,trash::orig-path",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            ) {
                while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
                    let orig = info
                        .attribute_byte_string("trash::orig-path")
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    if orig == *path {
                        let trash_file = trash.child(info.name());
                        match wayfinder::file_ops::restore_from_trash(&trash_file) {
                            Ok(_) => restored += 1,
                            Err(e) => {
                                log::error!("Failed to restore {}: {}", name, e);
                            }
                        }
                        break;
                    }
                }
            }
        }

        self.imp().last_trashed.borrow_mut().clear();

        if restored > 0 {
            // Reload directory to show restored files
            let current = self.imp().model.current_path();
            let _ = self.imp().model.load_directory(&current);
            self.update_status();

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
                    &format!("Restored {} files", restored),
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
                    &format!("Opened terminal in {}", path),
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
            ("Navigation", &[
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
            ]),
            ("File Operations", &[
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
                ("Delete", "Move to Bin"),
                ("Shift+Delete", "Delete permanently"),
                ("Ctrl+Shift+N", "New folder"),
                ("Ctrl+D", "Bookmark current folder"),
                ("Ctrl+Z", "Undo trash"),
            ]),
            ("View", &[
                ("Ctrl+1", "Grid view"),
                ("Ctrl+2", "List view"),
                ("Ctrl+H", "Toggle hidden files"),
                ("Ctrl+Shift+S", "Toggle sidebar"),
                ("Ctrl+F", "Search files"),
                ("Ctrl+I", "Properties"),
            ]),
            ("General", &[
                ("Ctrl+N", "New window"),
                ("Ctrl+`", "Open terminal here"),
                ("Menu or Shift+F10", "Context menu"),
                ("Tab", "Path completion (in location bar)"),
                ("Type letters", "Jump to matching file"),
                ("Ctrl+?", "This shortcuts window"),
            ]),
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
                row.update_property(&[gtk::accessible::Property::Label(
                    &format!("{}: {}", description, key),
                )]);

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
}
