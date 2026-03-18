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
                let announcement = format!("Opened {}, {} items", dir_name, count);
                self.announce(&announcement, AccessibleAnnouncementPriority::Medium);

                // Reset column, selection state, and focus the first item
                imp.current_column.set(0);
                imp.file_selection.borrow_mut().clear();
                imp.selection.set_selected(0);

                self.focus_current_view();
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
                let announcement = format!("Opened Bin, {} items", count);
                self.announce(&announcement, AccessibleAnnouncementPriority::Medium);

                imp.current_column.set(0);
                imp.selection.set_selected(0);
                self.focus_current_view();
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
                if let Err(e) = app.launch(&[gio_file], gio::AppLaunchContext::NONE) {
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
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
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
        let gio_file = gio::File::for_uri(&format!("trash:///{}", file.name()));
        match wayfinder::file_ops::restore_from_trash(&gio_file) {
            Ok(dest) => {
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

    pub fn copy_selected(&self) {
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

    pub fn cut_selected(&self) {
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

    pub fn paste(&self) {
        let imp = self.imp();
        let clipboard = imp.clipboard.borrow().clone();
        if let Some(state) = clipboard {
            let dest_dir = gio::File::for_path(imp.model.current_path());
            let parent_window: gtk::Window = self.clone().upcast();

            for source in &state.files {
                match state.operation {
                    ClipboardOperation::Copy => {
                        wayfinder::file_ops::copy_with_progress(source, &dest_dir, &parent_window);
                    }
                    ClipboardOperation::Cut => {
                        wayfinder::file_ops::move_with_progress(source, &dest_dir, &parent_window);
                    }
                }
            }

            // Clear clipboard after cut
            if state.operation == ClipboardOperation::Cut {
                *imp.clipboard.borrow_mut() = None;
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
        let mut success = 0;
        for file in &files {
            let gio_file = gio::File::for_path(file.path());
            if wayfinder::file_ops::trash_file(&gio_file).is_ok() {
                success += 1;
            }
        }
        self.imp().file_selection.borrow_mut().clear();
        self.update_status();
        if success == 1 {
            self.announce(
                &format!("Moved {} to Bin", files[0].name()),
                AccessibleAnnouncementPriority::Medium,
            );
        } else {
            self.announce(
                &format!("Moved {} files to Bin", success),
                AccessibleAnnouncementPriority::Medium,
            );
        }
    }

    pub fn delete_selected(&self) {
        let Some(file) = self.get_selected_file() else {
            return;
        };

        let window = self.clone();
        let dialog = gtk::AlertDialog::builder()
            .message(format!("Permanently delete {}?", file.name()))
            .detail("This cannot be undone.")
            .buttons(["Cancel", "Delete permanently"])
            .cancel_button(0)
            .default_button(0)
            .build();

        dialog.choose(
            Some(&window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(choice) = result {
                    if choice == 1 {
                        let gio_file = gio::File::for_path(file.path());
                        match wayfinder::file_ops::delete_file_recursive(&gio_file) {
                            Ok(()) => {
                                window.announce(
                                    &format!("Deleted {}", file.name()),
                                    AccessibleAnnouncementPriority::Medium,
                                );
                            }
                            Err(e) => {
                                window.announce(
                                    &format!("Failed to delete {}: {}", file.name(), e),
                                    AccessibleAnnouncementPriority::High,
                                );
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

        dlg.present();
        entry.grab_focus();
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

        dlg.present();
        entry.grab_focus();
    }
}
