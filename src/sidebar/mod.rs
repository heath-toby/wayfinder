mod imp;

use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

glib::wrapper! {
    pub struct WayfinderSidebar(ObjectSubclass<imp::SidebarInner>);
}

fn bookmarks_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gtk-3.0")
        .join("bookmarks")
}

pub fn load_bookmarks() -> Vec<(String, String)> {
    let file = bookmarks_file();
    let Ok(contents) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (uri, display) = if let Some((u, d)) = line.split_once(' ') {
                (u.to_string(), d.to_string())
            } else {
                let name = line
                    .strip_prefix("file://")
                    .unwrap_or(line)
                    .rsplit('/')
                    .next()
                    .unwrap_or("Bookmark")
                    .to_string();
                (line.to_string(), name)
            };
            Some((uri, display))
        })
        .collect()
}

fn save_bookmarks(bookmarks: &[(String, String)]) {
    let file = bookmarks_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let contents: String = bookmarks
        .iter()
        .map(|(uri, name)| format!("{} {}", uri, name))
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = std::fs::write(&file, contents) {
        log::warn!("Failed to save bookmarks: {}", e);
    }
}

pub fn add_bookmark(path: &str) -> bool {
    let uri = format!("file://{}", path);
    let mut bookmarks = load_bookmarks();
    if bookmarks.iter().any(|(u, _)| *u == uri) {
        return false; // already exists
    }
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    bookmarks.push((uri, name));
    save_bookmarks(&bookmarks);
    true
}

pub fn remove_bookmark(uri: &str) {
    let mut bookmarks = load_bookmarks();
    bookmarks.retain(|(u, _)| u != uri);
    save_bookmarks(&bookmarks);
}

struct PlaceEntry {
    name: &'static str,
    icon: &'static str,
    dir_fn: fn() -> Option<std::path::PathBuf>,
}

const PLACES: &[PlaceEntry] = &[
    PlaceEntry { name: "Home", icon: "user-home-symbolic", dir_fn: dirs::home_dir },
    PlaceEntry { name: "Desktop", icon: "user-desktop-symbolic", dir_fn: dirs::desktop_dir },
    PlaceEntry { name: "Documents", icon: "folder-documents-symbolic", dir_fn: dirs::document_dir },
    PlaceEntry { name: "Downloads", icon: "folder-download-symbolic", dir_fn: dirs::download_dir },
    PlaceEntry { name: "Music", icon: "folder-music-symbolic", dir_fn: dirs::audio_dir },
    PlaceEntry { name: "Pictures", icon: "folder-pictures-symbolic", dir_fn: dirs::picture_dir },
    PlaceEntry { name: "Videos", icon: "folder-videos-symbolic", dir_fn: dirs::video_dir },
];

impl WayfinderSidebar {
    pub fn new() -> Self {
        let sidebar: Self = glib::Object::builder().build();
        sidebar.populate_places();
        sidebar.connect_volume_signals();
        sidebar.connect_bookmark_delete_key();
        sidebar
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.imp().container
    }

    pub fn connect_place_activated<F: Fn(String) + 'static>(&self, callback: F) {
        self.imp().places_list.connect_row_activated(move |_list, row| {
            let name = row.widget_name();
            if let Some(path) = name.strip_prefix("place:") {
                callback(path.to_string());
            } else if let Some(path) = name.strip_prefix("bookmark:") {
                callback(path.to_string());
            } else if let Some(path) = name.strip_prefix("volume:") {
                callback(path.to_string());
            } else if name.starts_with("volume-unmounted:") {
                // Unmounted volume — try to mount it
                if let Some(uuid) = name.strip_prefix("volume-unmounted:") {
                    let vm = gio::VolumeMonitor::get();
                    for vol in vm.volumes() {
                        let vol_id = vol.uuid().map(|u| u.to_string()).unwrap_or_default();
                        if vol_id == uuid {
                            let mount_op = gio::MountOperation::new();
                            vol.mount(gio::MountMountFlags::NONE, Some(&mount_op), gio::Cancellable::NONE, |result| {
                                if let Err(e) = result {
                                    log::error!("Failed to mount volume: {}", e);
                                }
                            });
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Delete key removes the focused bookmark. Ctrl+Up/Down reorders bookmarks.
    fn connect_bookmark_delete_key(&self) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let list = self.imp().places_list.clone();
        let sidebar = self.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;

            let Some(row) = list.selected_row() else {
                return glib::Propagation::Proceed;
            };
            let name = row.widget_name().to_string();

            // Delete key: remove bookmark
            if key == gdk::Key::Delete {
                let Some(path) = name.strip_prefix("bookmark:") else {
                    return glib::Propagation::Proceed;
                };
                let uri = format!("file://{}", path);
                remove_bookmark(&uri);
                let idx = row.index();
                list.remove(&row);
                if idx > 0 {
                    if let Some(prev) = list.row_at_index(idx - 1) {
                        list.select_row(Some(&prev));
                        prev.grab_focus();
                    }
                }
                return glib::Propagation::Stop;
            }

            // Ctrl+Up/Down: reorder bookmarks
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            if !ctrl || !name.starts_with("bookmark:") {
                return glib::Propagation::Proceed;
            }
            if key != gdk::Key::Up && key != gdk::Key::Down {
                return glib::Propagation::Proceed;
            }

            // Find the neighbour bookmark in the requested direction
            let target_widget = if key == gdk::Key::Up {
                // Walk backwards to find previous bookmark row
                let mut prev = row.prev_sibling();
                loop {
                    match prev {
                        Some(ref w) if w.widget_name().starts_with("bookmark:") => break,
                        Some(ref w) => prev = w.prev_sibling(),
                        None => break,
                    }
                }
                prev
            } else {
                // Walk forward to find next bookmark row
                let mut next = row.next_sibling();
                loop {
                    match next {
                        Some(ref w) if w.widget_name().starts_with("bookmark:") => break,
                        Some(ref w) if w.widget_name() == "bookmark-separator" => {
                            next = w.next_sibling();
                        }
                        _ => break,
                    }
                }
                // Only proceed if we found another bookmark
                next.filter(|w| w.widget_name().starts_with("bookmark:"))
            };

            if target_widget.is_none() {
                // Announce boundary
                let msg = if key == gdk::Key::Up { "Already at top" } else { "Already at bottom" };
                list.announce(msg, gtk::AccessibleAnnouncementPriority::Medium);
                return glib::Propagation::Stop;
            }

            // Swap in the bookmarks file
            let mut bookmarks = load_bookmarks();
            let current_uri = format!("file://{}", name.strip_prefix("bookmark:").unwrap_or(""));
            let target_name = target_widget.as_ref().unwrap().widget_name().to_string();
            let target_uri = format!("file://{}", target_name.strip_prefix("bookmark:").unwrap_or(""));

            let cur_idx = bookmarks.iter().position(|(u, _)| *u == current_uri);
            let tgt_idx = bookmarks.iter().position(|(u, _)| *u == target_uri);

            if let (Some(ci), Some(ti)) = (cur_idx, tgt_idx) {
                // Get the neighbour's display name for announcement
                let neighbour_display = bookmarks[ti].1.clone();
                let announcement = if key == gdk::Key::Up {
                    format!("Moved above {}", neighbour_display)
                } else {
                    format!("Moved below {}", neighbour_display)
                };

                bookmarks.swap(ci, ti);
                save_bookmarks(&bookmarks);

                // Refresh and re-focus
                sidebar.refresh_bookmarks();

                // Find and focus the moved bookmark row, with announcement as label
                let mut child = list.first_child();
                while let Some(widget) = child {
                    if widget.widget_name() == name {
                        if let Some(r) = widget.downcast_ref::<gtk::ListBoxRow>() {
                            r.update_property(&[
                                gtk::accessible::Property::Label(&announcement),
                            ]);
                            list.select_row(Some(r));
                            r.grab_focus();
                            // Restore real label after Orca reads the announcement
                            let row_ref = r.clone();
                            let current_display = bookmarks[ci].1.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(500),
                                move || {
                                    row_ref.update_property(&[
                                        gtk::accessible::Property::Label(
                                            &format!("Bookmark: {}", current_display),
                                        ),
                                    ]);
                                },
                            );
                        }
                        break;
                    }
                    child = widget.next_sibling();
                }
            }

            glib::Propagation::Stop
        });
        self.imp().places_list.add_controller(controller);
    }

    pub fn refresh_bookmarks(&self) {
        let list = &self.imp().places_list;

        // Remove existing bookmark rows and bookmark separator
        let mut child = list.first_child();
        let mut to_remove = Vec::new();
        while let Some(widget) = child {
            let name = widget.widget_name().to_string();
            if name.starts_with("bookmark:") || name == "bookmark-separator" {
                to_remove.push(widget.clone());
            }
            child = widget.next_sibling();
        }
        for widget in to_remove {
            list.remove(&widget);
        }

        let bookmarks = load_bookmarks();
        if bookmarks.is_empty() {
            return;
        }

        // Find "File System" row position to insert after it
        let mut fs_pos: i32 = -1;
        let mut pos: i32 = 0;
        let mut child = list.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == "place:/" {
                fs_pos = pos;
                break;
            }
            pos += 1;
            child = widget.next_sibling();
        }

        let insert_pos = if fs_pos >= 0 { fs_pos + 1 } else {
            // Count all children and insert before the end
            let mut count: i32 = 0;
            let mut c = list.first_child();
            while c.is_some() {
                count += 1;
                c = c.unwrap().next_sibling();
            }
            count
        };

        // Insert bookmark separator
        let sep_row = gtk::ListBoxRow::new();
        sep_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        sep_row.set_selectable(false);
        sep_row.set_activatable(false);
        sep_row.set_widget_name("bookmark-separator");
        list.insert(&sep_row, insert_pos);

        // Insert bookmark rows
        for (i, (uri, display_name)) in bookmarks.iter().enumerate() {
            let path = uri.strip_prefix("file://").unwrap_or(uri);

            let icon = gtk::Image::from_icon_name("folder-symbolic");
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(display_name.as_str())
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&label);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(&format!("bookmark:{}", path));
            row.update_property(&[
                gtk::accessible::Property::Label(&format!("Bookmark: {}", display_name)),
            ]);

            // Right-click to remove
            let click = gtk::GestureClick::new();
            click.set_button(3);
            let uri_clone = uri.clone();
            let list_ref = list.clone();
            let row_ref = row.clone();
            click.connect_pressed(move |gesture, _n, _x, _y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                remove_bookmark(&uri_clone);
                list_ref.remove(&row_ref);
            });
            row.add_controller(click);

            list.insert(&row, insert_pos + 1 + i as i32);
        }
    }

    /// Refresh the volumes section in the sidebar using GIO VolumeMonitor.
    pub fn refresh_volumes(&self) {
        let list = &self.imp().places_list;

        // Remove existing volume rows and volume separator
        let mut child = list.first_child();
        let mut to_remove = Vec::new();
        while let Some(widget) = child {
            let name = widget.widget_name().to_string();
            if name.starts_with("volume:") || name.starts_with("volume-unmounted:") || name == "volume-separator" {
                to_remove.push(widget.clone());
            }
            child = widget.next_sibling();
        }
        for widget in to_remove {
            list.remove(&widget);
        }

        let vm = gio::VolumeMonitor::get();
        let mut volume_rows: Vec<(String, String, String, bool)> = Vec::new(); // (widget_name, label, icon_name, can_eject)

        // Mounted volumes
        for mount in vm.mounts() {
            let name = mount.name().to_string();
            let root = mount.default_location();
            let path = root.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();

            // Skip root filesystem and home — already in places
            if path == "/" || path == dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default() {
                continue;
            }

            let icon_name = "drive-removable-media-symbolic".to_string();

            let can_eject = mount.can_eject() || mount.can_unmount();
            volume_rows.push((format!("volume:{}", path), name, icon_name, can_eject));
        }

        // Unmounted volumes (that have no mount)
        for vol in vm.volumes() {
            if vol.get_mount().is_some() {
                continue; // already shown as mounted
            }
            let name = vol.name().to_string();
            let uuid = vol.uuid().map(|u| u.to_string()).unwrap_or_default();
            if uuid.is_empty() {
                continue;
            }
            let icon_name = "drive-removable-media-symbolic".to_string();

            volume_rows.push((format!("volume-unmounted:{}", uuid), format!("{} (unmounted)", name), icon_name, false));
        }

        if volume_rows.is_empty() {
            return;
        }

        // Find the Bin row and insert before its preceding separator.
        // The Bin separator is the unnamed one just before "place:trash:///".
        let mut insert_pos: i32 = 0;
        let mut pos: i32 = 0;
        let mut child = list.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == "place:trash:///" {
                // Insert before the separator that precedes Bin (one row back)
                insert_pos = (pos - 1).max(0);
                break;
            }
            pos += 1;
            insert_pos = pos;
            child = widget.next_sibling();
        }

        // Insert volume separator
        let sep_row = gtk::ListBoxRow::new();
        sep_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        sep_row.set_selectable(false);
        sep_row.set_activatable(false);
        sep_row.set_widget_name("volume-separator");
        list.insert(&sep_row, insert_pos);

        // Insert volume rows
        for (i, (widget_name, label, icon_name, can_eject)) in volume_rows.iter().enumerate() {
            let icon = gtk::Image::from_icon_name(icon_name.as_str());
            icon.set_pixel_size(16);

            let name_label = gtk::Label::builder()
                .label(label.as_str())
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&name_label);

            // Eject button for removable/unmountable volumes
            if *can_eject {
                let eject_btn = gtk::Button::from_icon_name("media-eject-symbolic");
                eject_btn.add_css_class("flat");
                eject_btn.set_tooltip_text(Some(&format!("Eject {}", label)));
                eject_btn.update_property(&[
                    gtk::accessible::Property::Label(&format!("Eject {}", label)),
                ]);

                // Find the mount path to identify which mount to eject
                let mount_path = widget_name.strip_prefix("volume:").unwrap_or("").to_string();
                let list_clone = list.clone();
                eject_btn.connect_clicked(move |_| {
                    let vm = gio::VolumeMonitor::get();
                    for mount in vm.mounts() {
                        let root = mount.default_location();
                        let p = root.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                        if p == mount_path {
                            if mount.can_eject() {
                                mount.eject_with_operation(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, |result| {
                                    if let Err(e) = result {
                                        log::error!("Failed to eject: {}", e);
                                    }
                                });
                            } else if mount.can_unmount() {
                                mount.unmount_with_operation(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, |result| {
                                    if let Err(e) = result {
                                        log::error!("Failed to unmount: {}", e);
                                    }
                                });
                            }
                            break;
                        }
                    }
                    // Remove the row after ejecting — will reappear on next signal
                    let _ = &list_clone; // keep reference alive
                });

                row_box.append(&eject_btn);
            }

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(widget_name);
            row.update_property(&[
                gtk::accessible::Property::Label(&format!("Volume: {}", label)),
            ]);

            list.insert(&row, insert_pos + 1 + i as i32);
        }
    }

    /// Connect to GIO VolumeMonitor signals for dynamic updates.
    pub fn connect_volume_signals(&self) {
        let vm = &self.imp().volume_monitor;

        let sidebar = self.clone();
        vm.connect_mount_added(move |_, _| {
            sidebar.refresh_volumes();
        });

        let sidebar = self.clone();
        vm.connect_mount_removed(move |_, _| {
            sidebar.refresh_volumes();
        });

        let sidebar = self.clone();
        vm.connect_volume_added(move |_, _| {
            sidebar.refresh_volumes();
        });

        let sidebar = self.clone();
        vm.connect_volume_removed(move |_, _| {
            sidebar.refresh_volumes();
        });
    }

    pub fn connect_trash_right_click<F: Fn() + 'static>(&self, callback: F) {
        let callback = std::rc::Rc::new(callback);
        // Walk through rows to find the trash row and add a right-click controller
        let mut child = self.imp().places_list.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == "place:trash:///" {
                let click = gtk::GestureClick::new();
                click.set_button(3);
                let cb = callback.clone();
                click.connect_pressed(move |gesture, _n, _x, _y| {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    cb();
                });
                widget.add_controller(click);
                break;
            }
            child = widget.next_sibling();
        }
    }

    pub fn rebuild(&self) {
        let list = &self.imp().places_list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        self.populate_places();
    }

    pub fn connect_sidebar_edit_menu(&self, parent_window: &gtk::Window) {
        let click = gtk::GestureClick::new();
        click.set_button(3); // right click
        let sidebar = self.clone();
        let parent = parent_window.clone();
        let list = self.imp().places_list.clone();
        click.connect_pressed(move |gesture, _n, x, y| {
            // Don't claim the event if it's on a bookmark or trash row (they have their own menus)
            if let Some(row) = list.row_at_y(y as i32) {
                let name = row.widget_name().to_string();
                if name.starts_with("bookmark:") || name == "place:trash:///" {
                    return;
                }
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);

            let popover = gtk::Popover::new();
            popover.set_parent(&list);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.set_has_arrow(false);

            let btn = gtk::Button::with_label("Edit Sidebar");
            btn.add_css_class("flat");
            btn.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

            let s = sidebar.clone();
            let p = parent.clone();
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                s.show_edit_dialog(&p);
            });

            popover.set_child(Some(&btn));
            popover.connect_closed(|pop| pop.unparent());
            popover.popup();
        });
        self.imp().places_list.add_controller(click);

        // Also handle Menu key and Shift+F10 for keyboard-triggered context menu
        let key_ctrl = gtk::EventControllerKey::new();
        let sidebar2 = self.clone();
        let parent2 = parent_window.clone();
        let list2 = self.imp().places_list.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;
            let is_menu = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK));

            if !is_menu {
                return glib::Propagation::Proceed;
            }

            // Check what row is focused — if it's a bookmark, let the delete key handler deal with it
            if let Some(row) = list2.selected_row() {
                let name = row.widget_name().to_string();
                if name.starts_with("bookmark:") {
                    // Show a popover with "Remove Bookmark" and "Edit Sidebar"
                    let popover = gtk::Popover::new();
                    popover.set_parent(&row);
                    popover.set_has_arrow(false);

                    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    vbox.add_css_class("menu");

                    let uri = format!("file://{}", name.strip_prefix("bookmark:").unwrap_or(""));

                    let remove_btn = gtk::Button::with_label("Remove Bookmark");
                    remove_btn.add_css_class("flat");
                    let pop = popover.clone();
                    let list_ref = list2.clone();
                    let row_ref = row.clone();
                    remove_btn.connect_clicked(move |_| {
                        pop.popdown();
                        remove_bookmark(&uri);
                        list_ref.remove(&row_ref);
                    });
                    vbox.append(&remove_btn);

                    let edit_btn = gtk::Button::with_label("Edit Sidebar");
                    edit_btn.add_css_class("flat");
                    let s = sidebar2.clone();
                    let p = parent2.clone();
                    let pop2 = popover.clone();
                    edit_btn.connect_clicked(move |_| {
                        pop2.popdown();
                        s.show_edit_dialog(&p);
                    });
                    vbox.append(&edit_btn);

                    popover.set_child(Some(&vbox));
                    popover.connect_closed(|pop| pop.unparent());
                    popover.popup();
                    return glib::Propagation::Stop;
                }
            }

            // For all other rows, show "Edit Sidebar"
            let popover = gtk::Popover::new();
            let anchor = list2.selected_row()
                .map(|r| r.upcast::<gtk::Widget>())
                .unwrap_or_else(|| list2.clone().upcast());
            popover.set_parent(&anchor);
            popover.set_has_arrow(false);

            let btn = gtk::Button::with_label("Edit Sidebar");
            btn.add_css_class("flat");
            btn.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

            let s = sidebar2.clone();
            let p = parent2.clone();
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                s.show_edit_dialog(&p);
            });

            popover.set_child(Some(&btn));
            popover.connect_closed(|pop| pop.unparent());
            popover.popup();
            glib::Propagation::Stop
        });
        self.imp().places_list.add_controller(key_ctrl);
    }

    pub fn show_edit_dialog(&self, parent: &gtk::Window) {
        let dlg = gtk::Window::builder()
            .title("Edit Sidebar")
            .modal(true)
            .transient_for(parent)
            .default_width(350)
            .default_height(450)
            .build();
        dlg.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

        // Load current config
        let config = crate::state::load_sidebar_config();

        // All available default places with their paths
        let mut all_places: Vec<(&str, &str, String)> = Vec::new();
        for place in PLACES {
            if let Some(path) = (place.dir_fn)() {
                all_places.push((place.name, place.icon, path.to_string_lossy().to_string()));
            }
        }
        all_places.push(("File System", "drive-harddisk-symbolic", "/".to_string()));

        // Determine order and visibility
        struct EditRow {
            id: String,
            icon: String,
            visible: bool,
        }

        let rows: Vec<EditRow> = if let Some(ref config) = config {
            let mut result: Vec<EditRow> = Vec::new();
            for entry in config {
                if let Some((_name, icon, _path)) = all_places.iter().find(|(n, _, _)| *n == entry.id) {
                    result.push(EditRow {
                        id: entry.id.clone(),
                        icon: icon.to_string(),
                        visible: entry.visible,
                    });
                }
            }
            // Add any places not in config
            for (name, icon, _) in &all_places {
                if !result.iter().any(|r| r.id == *name) {
                    result.push(EditRow {
                        id: name.to_string(),
                        icon: icon.to_string(),
                        visible: true,
                    });
                }
            }
            result
        } else {
            all_places.iter().map(|(name, icon, _)| EditRow {
                id: name.to_string(),
                icon: icon.to_string(),
                visible: true,
            }).collect()
        };

        // Build the dialog UI
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);

        let instructions = gtk::Label::builder()
            .label("Toggle places on or off. Use Ctrl+Up/Down to reorder.")
            .xalign(0.0)
            .wrap(true)
            .build();
        vbox.append(&instructions);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();

        let edit_list = gtk::ListBox::new();
        edit_list.set_selection_mode(gtk::SelectionMode::Single);
        edit_list.update_property(&[gtk::accessible::Property::Label("Sidebar places")]);

        let row_data: std::rc::Rc<std::cell::RefCell<Vec<(String, bool)>>> =
            std::rc::Rc::new(std::cell::RefCell::new(
                rows.iter().map(|r| (r.id.clone(), r.visible)).collect(),
            ));

        for row_info in &rows {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(8);
            hbox.set_margin_end(8);

            let check = gtk::CheckButton::new();
            check.set_active(row_info.visible);
            check.update_property(&[gtk::accessible::Property::Label(
                &format!("Show {}", row_info.id),
            )]);

            let icon = gtk::Image::from_icon_name(&row_info.icon);
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(&row_info.id)
                .xalign(0.0)
                .hexpand(true)
                .build();

            hbox.append(&check);
            hbox.append(&icon);
            hbox.append(&label);

            let list_row = gtk::ListBoxRow::new();
            list_row.set_child(Some(&hbox));
            list_row.set_widget_name(&row_info.id);
            list_row.update_property(&[gtk::accessible::Property::Label(&row_info.id)]);

            // Sync checkbox with data
            let data = row_data.clone();
            let id = row_info.id.clone();
            check.connect_toggled(move |cb| {
                let mut d = data.borrow_mut();
                if let Some(entry) = d.iter_mut().find(|(name, _)| name == &id) {
                    entry.1 = cb.is_active();
                }
            });

            edit_list.append(&list_row);
        }

        scrolled.set_child(Some(&edit_list));
        vbox.append(&scrolled);

        // Keyboard controller for reordering with announcements
        let key_ctrl = gtk::EventControllerKey::new();
        let list_for_keys = edit_list.clone();
        let data_for_keys = row_data.clone();
        let dlg_for_keys = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);

            if !ctrl {
                return glib::Propagation::Proceed;
            }

            let is_move = matches!(
                (key, shift),
                (gdk::Key::Up, false) | (gdk::Key::Down, false) |
                (gdk::Key::Home, true) | (gdk::Key::End, true)
            );
            if !is_move {
                return glib::Propagation::Proceed;
            }

            let Some(selected) = list_for_keys.selected_row() else {
                return glib::Propagation::Proceed;
            };
            let idx = selected.index();
            let n = data_for_keys.borrow().len() as i32;

            // Check boundaries and announce if can't move
            match (key, shift) {
                (gdk::Key::Up, false) if idx == 0 => {
                    dlg_for_keys.announce("Already at top", gtk::AccessibleAnnouncementPriority::Medium);
                    return glib::Propagation::Stop;
                }
                (gdk::Key::Down, false) if idx >= n - 1 => {
                    dlg_for_keys.announce("Already at bottom", gtk::AccessibleAnnouncementPriority::Medium);
                    return glib::Propagation::Stop;
                }
                (gdk::Key::Home, true) if idx == 0 => {
                    dlg_for_keys.announce("Already at top", gtk::AccessibleAnnouncementPriority::Medium);
                    return glib::Propagation::Stop;
                }
                (gdk::Key::End, true) if idx >= n - 1 => {
                    dlg_for_keys.announce("Already at bottom", gtk::AccessibleAnnouncementPriority::Medium);
                    return glib::Propagation::Stop;
                }
                _ => {}
            }

            let new_idx = match (key, shift) {
                (gdk::Key::Up, false) => idx - 1,
                (gdk::Key::Down, false) => idx + 1,
                (gdk::Key::Home, true) => 0,
                (gdk::Key::End, true) => n - 1,
                _ => unreachable!(),
            };

            // Compute the announcement before rebuilding
            let announcement = {
                let d = data_for_keys.borrow();
                let neighbour_name = if key == gdk::Key::Up || (key == gdk::Key::Home && shift) {
                    // Will move above — neighbour is the one currently at new_idx
                    d.get(new_idx as usize).map(|(name, _)| name.clone()).unwrap_or_default()
                } else {
                    // Will move below — neighbour is the one currently at new_idx
                    d.get(new_idx as usize).map(|(name, _)| name.clone()).unwrap_or_default()
                };
                match (key, shift) {
                    (gdk::Key::Home, true) => "Moved to top".to_string(),
                    (gdk::Key::End, true) => "Moved to bottom".to_string(),
                    (gdk::Key::Up, _) => format!("Moved above {}", neighbour_name),
                    (gdk::Key::Down, _) => format!("Moved below {}", neighbour_name),
                    _ => String::new(),
                }
            };

            // Move in data: remove from old position, insert at new
            {
                let mut d = data_for_keys.borrow_mut();
                let item = d.remove(idx as usize);
                d.insert(new_idx as usize, item);
            }

            // Rebuild the list visually to keep everything in sync
            while let Some(child) = list_for_keys.first_child() {
                list_for_keys.remove(&child);
            }
            let data = data_for_keys.borrow();
            for (i, (id, vis)) in data.iter().enumerate() {
                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                hbox.set_margin_top(4);
                hbox.set_margin_bottom(4);
                hbox.set_margin_start(8);
                hbox.set_margin_end(8);

                let check = gtk::CheckButton::new();
                check.set_active(*vis);
                check.update_property(&[gtk::accessible::Property::Label(
                    &format!("Show {}", id),
                )]);

                let icon_name = PLACES.iter()
                    .find(|p| p.name == id)
                    .map(|p| p.icon)
                    .unwrap_or(if id == "File System" { "drive-harddisk-symbolic" } else { "folder-symbolic" });
                let icon = gtk::Image::from_icon_name(icon_name);
                icon.set_pixel_size(16);

                let label = gtk::Label::builder()
                    .label(id.as_str())
                    .xalign(0.0)
                    .hexpand(true)
                    .build();

                hbox.append(&check);
                hbox.append(&icon);
                hbox.append(&label);

                let list_row = gtk::ListBoxRow::new();
                list_row.set_child(Some(&hbox));
                list_row.set_widget_name(id);

                // For the moved row, set the announcement as the label
                // so Orca reads it instead of the plain name
                if i == new_idx as usize {
                    list_row.update_property(&[gtk::accessible::Property::Label(
                        &announcement,
                    )]);
                } else {
                    list_row.update_property(&[gtk::accessible::Property::Label(id.as_str())]);
                }

                // Sync checkbox
                let data_ref = data_for_keys.clone();
                let id_clone = id.clone();
                check.connect_toggled(move |cb| {
                    let mut d = data_ref.borrow_mut();
                    if let Some(entry) = d.iter_mut().find(|(name, _)| name == &id_clone) {
                        entry.1 = cb.is_active();
                    }
                });

                list_for_keys.append(&list_row);

                if i == new_idx as usize {
                    list_for_keys.select_row(Some(&list_row));
                    // Restore the real label after Orca has read the announcement
                    let row_ref = list_row.clone();
                    let real_name = id.clone();
                    let row_to_focus = list_row.clone();
                    glib::idle_add_local_once(move || {
                        row_to_focus.grab_focus();
                    });
                    glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                        row_ref.update_property(&[gtk::accessible::Property::Label(
                            real_name.as_str(),
                        )]);
                    });
                }
            }
            drop(data);

            glib::Propagation::Stop
        });
        edit_list.add_controller(key_ctrl);

        // Done button
        let done_btn = gtk::Button::with_label("Done");
        done_btn.set_halign(gtk::Align::End);
        done_btn.set_margin_top(8);
        vbox.append(&done_btn);

        dlg.set_child(Some(&vbox));

        let sidebar = self.clone();
        let d = dlg.clone();
        let data_for_save = row_data.clone();
        done_btn.connect_clicked(move |_| {
            // Save config
            let data = data_for_save.borrow();
            let entries: Vec<crate::state::SidebarEntry> = data
                .iter()
                .map(|(id, vis)| crate::state::SidebarEntry {
                    id: id.clone(),
                    visible: *vis,
                })
                .collect();
            crate::state::save_sidebar_config(&entries);

            d.close();

            // Rebuild sidebar
            sidebar.rebuild();
        });

        dlg.present();
        // Focus first row
        if let Some(first) = edit_list.row_at_index(0) {
            edit_list.select_row(Some(&first));
            first.grab_focus();
        }
    }

    fn populate_places(&self) {
        let list = &self.imp().places_list;

        // All available default places with their paths
        let mut all_places: Vec<(&str, &str, String)> = Vec::new();
        for place in PLACES {
            if let Some(path) = (place.dir_fn)() {
                all_places.push((place.name, place.icon, path.to_string_lossy().to_string()));
            }
        }
        all_places.push(("File System", "drive-harddisk-symbolic", "/".to_string()));

        // Determine order and visibility from config
        let config = crate::state::load_sidebar_config();

        let ordered: Vec<(&str, &str, String, bool)> = if let Some(ref config) = config {
            let mut result = Vec::new();
            // Config-ordered entries first
            for entry in config {
                if let Some((name, icon, path)) = all_places.iter().find(|(n, _, _)| *n == entry.id) {
                    result.push((*name, *icon, path.clone(), entry.visible));
                }
            }
            // Any new places not in config
            for (name, icon, path) in &all_places {
                if !result.iter().any(|(n, _, _, _)| n == name) {
                    result.push((name, icon, path.clone(), true));
                }
            }
            result
        } else {
            all_places.iter().map(|(n, i, p)| (*n, *i, p.clone(), true)).collect()
        };

        for (name, icon_name, path, visible) in &ordered {
            if !visible {
                continue;
            }

            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(*name)
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&label);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(&format!("place:{}", path));
            row.update_property(&[gtk::accessible::Property::Label(name)]);

            list.append(&row);
        }

        // Bookmarks from ~/.config/gtk-3.0/bookmarks
        self.refresh_bookmarks();

        // Mounted volumes and drives
        self.refresh_volumes();

        // Separator before Bin
        let separator_row = gtk::ListBoxRow::new();
        separator_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        separator_row.set_selectable(false);
        separator_row.set_activatable(false);
        list.append(&separator_row);

        // Bin
        let icon = gtk::Image::from_icon_name("user-trash-symbolic");
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label("Bin")
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        row_box.append(&icon);
        row_box.append(&label);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_widget_name("place:trash:///");
        row.update_property(&[
            gtk::accessible::Property::Label("Bin"),
        ]);
        list.append(&row);
    }
}

impl Default for WayfinderSidebar {
    fn default() -> Self {
        Self::new()
    }
}
