use std::os::unix::fs::PermissionsExt;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::file_object::FileObject;

pub fn show_properties_dialog(file: &FileObject, parent: &gtk::Window) {
    let gio_file = gio::File::for_path(file.path());

    let info = match gio_file.query_info(
        "standard::*,time::modified",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    ) {
        Ok(info) => info,
        Err(e) => {
            parent.announce(
                &format!("Could not get properties: {}", e),
                AccessibleAnnouncementPriority::High,
            );
            return;
        }
    };

    let dlg = gtk::Window::builder()
        .title(format!("{} — Properties", file.name()))
        .modal(true)
        .transient_for(parent)
        .default_width(450)
        .default_height(500)
        .build();

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
    let modified = info
        .modification_date_time()
        .map(|dt: glib::DateTime| {
            dt.format("%Y-%m-%d %H:%M:%S")
                .unwrap_or_else(|_| glib::GString::from(""))
                .to_string()
        })
        .unwrap_or_else(|| "Unknown".to_string());
    add_info_row(&grid, row, "Modified", &modified);
    row += 1;

    // Created and Accessed from std::fs metadata
    if let Ok(metadata) = std::fs::metadata(file.path()) {
        if let Ok(created) = metadata.created() {
            let dt: chrono::DateTime<chrono::Local> = created.into();
            add_info_row(&grid, row, "Created", &dt.format("%Y-%m-%d %H:%M:%S").to_string());
            row += 1;
        }

        if let Ok(accessed) = metadata.accessed() {
            let dt: chrono::DateTime<chrono::Local> = accessed.into();
            add_info_row(&grid, row, "Last Opened", &dt.format("%Y-%m-%d %H:%M:%S").to_string());
            row += 1;
        }

        let mode = metadata.permissions().mode();
        let perm_string = format_permissions(mode);
        let octal = format!("{:o}", mode & 0o7777);
        add_info_row(
            &grid,
            row,
            "Permissions",
            &format!("{} ({})", perm_string, octal),
        );
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
            let string_list = gtk::StringList::new(&app_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());

            let dropdown = gtk::DropDown::new(Some(string_list), gtk::Expression::NONE);
            dropdown.set_hexpand(true);
            dropdown.update_property(&[gtk::accessible::Property::Label(
                "Open this file with",
            )]);

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
                            &format!("Failed to set default: {}", e),
                            AccessibleAnnouncementPriority::High,
                        );
                    } else {
                        let name = app.name();
                        parent_win.announce(
                            &format!("{} set as default for all {} files", name, ct),
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

fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ];
    for (bit, ch) in flags {
        if mode & bit != 0 {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}
