use std::os::unix::fs::PermissionsExt;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::file_object::FileObject;

/// What the dialog shows from the filesystem, gathered off the main thread
/// (the file may be on a slow mount).
struct Details {
    modified: String,
    created: Option<std::time::SystemTime>,
    accessed: Option<std::time::SystemTime>,
    /// `None` if the file's metadata couldn't be read; the permissions
    /// section is left out then.
    mode: Option<u32>,
}

/// setuid, setgid and sticky.
const SPECIAL_BITS: u32 = 0o7000;

fn gather_details(path: &str) -> Result<Details, String> {
    let info = gio::File::for_path(path)
        .query_info(
            "time::modified",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
    let modified = info
        .modification_date_time()
        .map(|dt: glib::DateTime| {
            // GIO gives UTC; show the time as it is here.
            dt.to_local()
                .unwrap_or(dt)
                .format("%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| glib::GString::from(""))
                .to_string()
        })
        .unwrap_or_else(|| "Unknown".to_string());
    let metadata = std::fs::metadata(path).ok();
    Ok(Details {
        modified,
        created: metadata.as_ref().and_then(|m| m.created().ok()),
        accessed: metadata.as_ref().and_then(|m| m.accessed().ok()),
        mode: metadata.map(|m| m.permissions().mode()),
    })
}

pub fn show_properties_dialog(file: &FileObject, parent: &gtk::Window) {
    let path = file.path();
    let file = file.clone();
    let parent = parent.clone();
    let parent_slow = parent.clone();
    let waiting = wayfinder::background::waiting_message(&path);
    wayfinder::background::run(
        move || gather_details(&path),
        move |details| match details {
            Ok(details) => build_dialog(&file, &parent, details),
            Err(e) => parent.announce(
                &format!("Could not get properties: {e}"),
                AccessibleAnnouncementPriority::High,
            ),
        },
        move || {
            if let Some(msg) = waiting {
                parent_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
            }
        },
    );
}

fn build_dialog(file: &FileObject, parent: &gtk::Window, details: Details) {
    let dlg = gtk::Window::builder()
        .title(format!("{} — Properties", file.name()))
        .modal(true)
        .transient_for(parent)
        .default_width(450)
        .default_height(500)
        .build();

    dlg.update_property(&[gtk::accessible::Property::Label(&format!(
        "Properties for {}",
        file.name()
    ))]);

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

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .build();

    let mut row = 0;

    add_info_row(&grid, row, "Name", &file.name());
    row += 1;

    add_info_row(&grid, row, "Kind", &file.file_type_name());
    row += 1;

    if !file.is_directory() {
        let size_detail = format!("{} ({} bytes)", file.size_display(), file.size());
        add_info_row(&grid, row, "Size", &size_detail);
        row += 1;
    }

    let parent_path = std::path::Path::new(&file.path())
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    add_info_row(&grid, row, "Location", &parent_path);
    row += 1;

    // Modified from GIO
    add_info_row(&grid, row, "Modified", &details.modified);
    row += 1;

    // Created and Accessed from std::fs metadata
    if let Some(mode) = details.mode {
        if let Some(created) = details.created {
            let dt: chrono::DateTime<chrono::Local> = created.into();
            add_info_row(
                &grid,
                row,
                "Created",
                &dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            );
            row += 1;
        }

        if let Some(accessed) = details.accessed {
            let dt: chrono::DateTime<chrono::Local> = accessed.into();
            add_info_row(
                &grid,
                row,
                "Last Opened",
                &dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            );
            row += 1;
        }

        let original_mode = mode;

        // --- Permissions section ---
        let perm_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        perm_sep.set_margin_top(8);
        perm_sep.set_margin_bottom(8);
        grid.attach(&perm_sep, 0, row, 2, 1);
        row += 1;

        let perm_heading = gtk::Label::builder()
            .label("Permissions")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        grid.attach(&perm_heading, 0, row, 2, 1);
        row += 1;

        // 3x3 permission grid
        let perm_grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(12)
            .build();

        // Header row
        for (col, header) in ["Read", "Write", "Execute"].iter().enumerate() {
            let label = gtk::Label::builder()
                .label(*header)
                .css_classes(["dim-label"])
                .build();
            perm_grid.attach(&label, (col + 1) as i32, 0, 1, 1);
        }

        // Guard flag to prevent infinite signal loops between checkboxes and octal entry
        let updating = std::rc::Rc::new(std::cell::RefCell::new(false));
        // setuid, setgid and sticky: no checkboxes, but kept (and settable
        // in the octal entry), never silently dropped.
        let special = std::rc::Rc::new(std::cell::Cell::new(mode & SPECIAL_BITS));

        // Permission bits: (bit, row_label, perm_name)
        let bit_layout: [(u32, &str, &str); 9] = [
            (0o400, "Owner", "read"),
            (0o200, "Owner", "write"),
            (0o100, "Owner", "execute"),
            (0o040, "Group", "read"),
            (0o020, "Group", "write"),
            (0o010, "Group", "execute"),
            (0o004, "Others", "read"),
            (0o002, "Others", "write"),
            (0o001, "Others", "execute"),
        ];

        let checkboxes: std::rc::Rc<Vec<(u32, gtk::CheckButton)>> = std::rc::Rc::new(
            bit_layout
                .iter()
                .enumerate()
                .map(|(i, (bit, row_label, perm_name))| {
                    let grid_row = (i / 3) as i32 + 1;
                    let grid_col = (i % 3) as i32 + 1;

                    // Row label (only for first column)
                    if grid_col == 1 {
                        let label = gtk::Label::builder()
                            .label(*row_label)
                            .xalign(1.0)
                            .css_classes(["dim-label"])
                            .build();
                        perm_grid.attach(&label, 0, grid_row, 1, 1);
                    }

                    let cb = gtk::CheckButton::new();
                    cb.set_active(mode & bit != 0);
                    cb.update_property(&[gtk::accessible::Property::Label(&format!(
                        "{row_label} {perm_name} permission"
                    ))]);
                    perm_grid.attach(&cb, grid_col, grid_row, 1, 1);
                    (*bit, cb)
                })
                .collect(),
        );

        // Octal entry
        let octal_label = gtk::Label::builder()
            .label("Octal")
            .xalign(1.0)
            .css_classes(["dim-label"])
            .build();
        let octal_entry = gtk::Entry::builder()
            .text(format!("{:o}", mode & 0o7777))
            .max_length(4)
            .width_chars(6)
            .build();
        octal_entry.update_property(&[gtk::accessible::Property::Label("Octal permissions")]);
        perm_grid.attach(&octal_label, 0, 4, 1, 1);
        perm_grid.attach(&octal_entry, 1, 4, 3, 1);

        // Wire checkboxes -> octal entry
        for (bit, cb) in checkboxes.iter() {
            let cbs = checkboxes.clone();
            let entry = octal_entry.clone();
            let guard = updating.clone();
            let special = special.clone();
            let _bit = *bit; // not used in closure but keeping for clarity
            cb.connect_toggled(move |_| {
                if *guard.borrow() {
                    return;
                }
                *guard.borrow_mut() = true;
                let bits: u32 = cbs
                    .iter()
                    .filter(|(_, c)| c.is_active())
                    .map(|(b, _)| b)
                    .sum();
                entry.set_text(&format!("{:o}", special.get() | bits));
                *guard.borrow_mut() = false;
            });
        }

        // Wire octal entry -> checkboxes
        {
            let cbs = checkboxes.clone();
            let guard = updating.clone();
            let special = special.clone();
            octal_entry.connect_changed(move |entry| {
                if *guard.borrow() {
                    return;
                }
                let text = entry.text().to_string();
                if let Ok(new_mode) = u32::from_str_radix(&text, 8) {
                    if new_mode <= 0o7777 {
                        special.set(new_mode & SPECIAL_BITS);
                        *guard.borrow_mut() = true;
                        for (bit, cb) in cbs.iter() {
                            cb.set_active(new_mode & bit != 0);
                        }
                        *guard.borrow_mut() = false;
                    }
                }
            });
        }

        grid.attach(&perm_grid, 0, row, 2, 1);
        row += 1;

        // Apply button
        let apply_btn = gtk::Button::with_label("Apply permissions");
        apply_btn.update_property(&[gtk::accessible::Property::Label("Apply file permissions")]);
        apply_btn.set_margin_top(4);
        let fp = file.path();
        let parent_ref = parent.clone();
        let cbs_apply = checkboxes.clone();
        let special_apply = special.clone();
        apply_btn.connect_clicked(move |_| {
            let bits: u32 = cbs_apply.iter()
                .filter(|(_, c)| c.is_active())
                .map(|(b, _)| b)
                .sum();
            let new_perm_bits = special_apply.get() | bits;
            // Preserve file type bits, only change permission bits
            let new_mode = (original_mode & !0o7777) | (new_perm_bits & 0o7777);
            let perms = std::fs::Permissions::from_mode(new_mode);
            let path = fp.clone();
            let parent_done = parent_ref.clone();
            let parent_slow = parent_ref.clone();
            let waiting = wayfinder::background::waiting_message(&fp);
            wayfinder::background::run(
                move || std::fs::set_permissions(&path, perms),
                move |result| match result {
                    Ok(()) => {
                        parent_done.announce(
                            &format!("Permissions changed to {new_perm_bits:o}"),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                    Err(e) => {
                        let msg = if e.kind() == std::io::ErrorKind::PermissionDenied {
                            "Permission denied. You must be the file owner or root to change permissions.".to_string()
                        } else {
                            format!("Failed to change permissions: {e}")
                        };
                        parent_done.announce(&msg, AccessibleAnnouncementPriority::High);
                    }
                },
                move || {
                    if let Some(msg) = waiting {
                        parent_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                    }
                },
            );
        });
        grid.attach(&apply_btn, 0, row, 2, 1);
        row += 1;
    }

    add_info_row(&grid, row, "MIME Type", &file.mime_type());
    row += 1;

    // Open With section (for files only)
    if !file.is_directory() && !file.mime_type().is_empty() {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(8);
        separator.set_margin_bottom(8);
        grid.attach(&separator, 0, row, 2, 1);
        row += 1;

        let open_with_label = gtk::Label::builder()
            .label("Open With")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        grid.attach(&open_with_label, 0, row, 2, 1);
        row += 1;

        let content_type = file.mime_type();
        let apps = gio::AppInfo::all_for_type(&content_type);
        let default_app = gio::AppInfo::default_for_type(&content_type, false);
        let default_name = default_app.as_ref().map(|a| a.name().to_string());

        if apps.is_empty() {
            add_info_row(&grid, row, "", "No applications found");
        } else {
            // Build a string list for the dropdown
            let app_names: Vec<String> = apps.iter().map(|a| a.name().to_string()).collect();
            let string_list =
                gtk::StringList::new(&app_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());

            let dropdown = gtk::DropDown::new(Some(string_list), gtk::Expression::NONE);
            dropdown.set_hexpand(true);
            dropdown.update_property(&[gtk::accessible::Property::Label("Open this file with")]);

            // Check for a per-file association first, then fall back to MIME default
            let file_path = file.path();
            let per_file_app = wayfinder::state::load_file_app(&file_path);
            let mut selected_idx = 0u32;

            if let Some(ref per_file_id) = per_file_app {
                // Find the app matching the per-file desktop ID
                for (i, app) in apps.iter().enumerate() {
                    if app.id().map(|id| id.to_string()) == Some(per_file_id.clone()) {
                        selected_idx = i as u32;
                        break;
                    }
                }
            } else if let Some(ref default_name) = default_name {
                for (i, name) in app_names.iter().enumerate() {
                    if name == default_name {
                        selected_idx = i as u32;
                        break;
                    }
                }
            }
            dropdown.set_selected(selected_idx);

            // When the dropdown changes, save the per-file association
            let apps_for_change = apps.clone();
            let fp = file_path.clone();
            let parent_for_change = parent.clone();
            dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                if let Some(app) = apps_for_change.get(idx) {
                    if let Some(id) = app.id() {
                        wayfinder::state::save_file_app(&fp, id.as_ref());
                        parent_for_change.announce(
                            &format!("{} will be used to open this file", app.name()),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                }
            });

            let label = gtk::Label::builder()
                .label("Open With")
                .xalign(1.0)
                .css_classes(["dim-label"])
                .build();
            grid.attach(&label, 0, row, 1, 1);
            grid.attach(&dropdown, 1, row, 1, 1);
            row += 1;

            // "Set as Default" sets the MIME type default for ALL files of this type
            let set_default_btn = gtk::Button::with_label("Set as Default for All");
            set_default_btn.update_property(&[gtk::accessible::Property::Label(
                "Set selected application as default for all files of this type",
            )]);
            let ct = content_type.clone();
            let parent_win = parent.clone();
            let apps_clone = apps.clone();
            let dd = dropdown.clone();
            set_default_btn.connect_clicked(move |_| {
                let idx = dd.selected() as usize;
                if let Some(app) = apps_clone.get(idx) {
                    if let Err(e) = app.set_as_default_for_type(&ct) {
                        parent_win.announce(
                            &format!("Failed to set default: {e}"),
                            AccessibleAnnouncementPriority::High,
                        );
                    } else {
                        let name = app.name();
                        parent_win.announce(
                            &format!("{name} set as default for all {ct} files"),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                }
            });
            grid.attach(&set_default_btn, 0, row, 2, 1);
        }
    }

    scrolled.set_child(Some(&grid));
    vbox.append(&scrolled);

    let close_btn = gtk::Button::with_label("Close");
    close_btn.set_margin_top(12);
    close_btn.set_halign(gtk::Align::End);
    let d = dlg.clone();
    close_btn.connect_clicked(move |_| {
        d.close();
    });
    vbox.append(&close_btn);

    dlg.set_child(Some(&vbox));
    dlg.present();
}

fn add_info_row(grid: &gtk::Grid, row: i32, label_text: &str, value: &str) {
    let label = gtk::Label::builder()
        .label(label_text)
        .xalign(1.0)
        .css_classes(["dim-label"])
        .build();

    let entry = gtk::Entry::builder()
        .text(value)
        .editable(false)
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(label_text)]);

    grid.attach(&label, 0, row, 1, 1);
    grid.attach(&entry, 1, row, 1, 1);
}
