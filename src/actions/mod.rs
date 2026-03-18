use std::path::{Path, PathBuf};

use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

/// A custom context menu action loaded from .desktop files or Nautilus scripts.
pub struct CustomAction {
    pub name: String,
    pub exec: String,
    pub mime_types: Vec<String>,
    pub is_nautilus_script: bool,
}

/// Load all custom actions from standard directories and bundled defaults.
/// User actions take priority. Actions with duplicate names are deduplicated
/// (first match wins), so user overrides beat system defaults.
pub fn load_actions() -> Vec<CustomAction> {
    let mut actions = Vec::new();

    // Scan directories in priority order (user first, system last)
    let dirs_to_scan: Vec<Option<PathBuf>> = vec![
        // User custom actions (highest priority)
        dirs::data_dir().map(|d| d.join("wayfinder/actions")),
        // FMA compatible actions (user + system)
        dirs::data_dir().map(|d| d.join("file-manager/actions")),
        Some(PathBuf::from("/usr/share/file-manager/actions")),
        // System-installed Wayfinder actions (for packaged installs)
        Some(PathBuf::from("/usr/share/wayfinder/actions")),
        Some(PathBuf::from("/usr/local/share/wayfinder/actions")),
        // Source tree (for development — cargo run from repo root)
        Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/data/actions"))),
    ];

    for dir in dirs_to_scan.into_iter().flatten() {
        if dir.is_dir() {
            actions.extend(load_desktop_actions(&dir));
        }
    }

    // Nautilus scripts
    if let Some(scripts_dir) = dirs::data_dir().map(|d| d.join("nautilus/scripts")) {
        if scripts_dir.is_dir() {
            actions.extend(load_nautilus_scripts(&scripts_dir));
        }
    }

    // Deduplicate by name — first occurrence wins
    let mut seen = std::collections::HashSet::new();
    actions.retain(|a| seen.insert(a.name.clone()));

    actions
}

/// Check if a command exists on PATH.
fn has_command(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}


/// Parse .desktop-like action files from a directory.
fn load_desktop_actions(dir: &Path) -> Vec<CustomAction> {
    let mut actions = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return actions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "desktop") {
            continue;
        }
        if let Some(action) = parse_desktop_action(&path) {
            actions.push(action);
        }
    }

    actions
}

/// Parse a single .desktop action file.
fn parse_desktop_action(path: &Path) -> Option<CustomAction> {
    let contents = std::fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut exec = None;
    let mut try_exec = None;
    let mut mime_types = Vec::new();
    let mut in_desktop_entry = false;

    for line in contents.lines() {
        let line = line.trim();
        if line == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }
        if line.starts_with('[') {
            in_desktop_entry = false;
            continue;
        }
        if !in_desktop_entry {
            continue;
        }

        if let Some(val) = line.strip_prefix("Name=") {
            name = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Exec=") {
            exec = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("TryExec=") {
            try_exec = Some(val.to_string());
        } else if let Some(val) = line
            .strip_prefix("MimeTypes=")
            .or_else(|| line.strip_prefix("MimeType="))
        {
            mime_types = val
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    // If TryExec is set, skip action if the command isn't found
    if let Some(ref cmd) = try_exec {
        if !has_command(cmd) {
            return None;
        }
    }

    Some(CustomAction {
        name: name?,
        exec: exec?,
        mime_types,
        is_nautilus_script: false,
    })
}

/// Load executable scripts from the Nautilus scripts directory.
fn load_nautilus_scripts(dir: &Path) -> Vec<CustomAction> {
    let mut actions = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return actions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Check if executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                if meta.permissions().mode() & 0o111 == 0 {
                    continue; // not executable
                }
            }
        }

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        if name.is_empty() {
            continue;
        }

        actions.push(CustomAction {
            name,
            exec: path.to_string_lossy().to_string(),
            mime_types: Vec::new(), // scripts match all types
            is_nautilus_script: true,
        });
    }

    actions
}

/// Check if an action matches a given MIME type.
pub fn matches_mime(action: &CustomAction, mime_type: &str) -> bool {
    if action.mime_types.is_empty() {
        return true; // no restriction = matches all
    }
    action.mime_types.iter().any(|pattern| {
        if pattern == mime_type {
            return true;
        }
        // Support wildcard like "application/*"
        if let Some(prefix) = pattern.strip_suffix("/*") {
            return mime_type.starts_with(prefix);
        }
        false
    })
}

/// Returns true if this action should show the compress dialog.
pub fn is_compress_dialog(action: &CustomAction) -> bool {
    action.exec == "__compress_dialog__"
}

/// Execute an action with the given file paths and current directory.
pub fn execute_action(action: &CustomAction, files: &[String], current_dir: &str) {
    if action.is_nautilus_script {
        execute_nautilus_script(action, files, current_dir);
    } else {
        execute_desktop_action(action, files);
    }
}

/// Show a compress dialog letting the user pick the archive format.
pub fn show_compress_dialog(files: &[String], parent_window: &gtk::Window) {
    let dlg = gtk::Window::builder()
        .title("Compress")
        .modal(true)
        .transient_for(parent_window)
        .default_width(400)
        .default_height(250)
        .build();
    dlg.update_property(&[gtk::accessible::Property::Label("Compress files")]);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    // Archive name entry
    let first_file = Path::new(files.first().map(|s| s.as_str()).unwrap_or("archive"));
    let default_stem = first_file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string());

    let name_label = gtk::Label::builder()
        .label("Archive name:")
        .xalign(0.0)
        .build();
    let name_entry = gtk::Entry::builder()
        .text(&default_stem)
        .hexpand(true)
        .build();
    name_entry.update_property(&[gtk::accessible::Property::Label("Archive name")]);

    vbox.append(&name_label);
    vbox.append(&name_entry);

    // Format dropdown
    let format_label = gtk::Label::builder()
        .label("Format:")
        .xalign(0.0)
        .build();

    let mut formats: Vec<(&str, &str)> = Vec::new();
    if has_command("zip") {
        formats.push(("Zip (.zip)", "zip"));
    }
    if has_command("bsdtar") || has_command("gzip") {
        formats.push(("tar.gz", "tar.gz"));
    }
    if has_command("bsdtar") || has_command("xz") {
        formats.push(("tar.xz", "tar.xz"));
    }
    if has_command("bsdtar") || has_command("zstd") {
        formats.push(("tar.zst", "tar.zst"));
    }
    if has_command("bsdtar") {
        formats.push(("tar (uncompressed)", "tar"));
    }
    if has_command("7z") {
        formats.push(("7-Zip (.7z)", "7z"));
    }

    let format_names: Vec<&str> = formats.iter().map(|(name, _)| *name).collect();
    let string_list = gtk::StringList::new(&format_names);
    let dropdown = gtk::DropDown::new(Some(string_list), gtk::Expression::NONE);
    dropdown.update_property(&[gtk::accessible::Property::Label("Archive format")]);

    vbox.append(&format_label);
    vbox.append(&dropdown);

    // Buttons
    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_box.set_halign(gtk::Align::End);
    button_box.set_margin_top(8);

    let cancel_btn = gtk::Button::with_label("Cancel");
    let create_btn = gtk::Button::with_label("Create");
    create_btn.add_css_class("suggested-action");

    button_box.append(&cancel_btn);
    button_box.append(&create_btn);
    vbox.append(&button_box);

    dlg.set_child(Some(&vbox));

    let d = dlg.clone();
    cancel_btn.connect_clicked(move |_| d.close());

    let d = dlg.clone();
    let files = files.to_vec();
    let formats_clone = formats.iter().map(|(_, ext)| ext.to_string()).collect::<Vec<_>>();
    let parent = parent_window.clone();
    let entry_for_focus = name_entry.clone();
    create_btn.connect_clicked(move |_| {
        let stem = name_entry.text().to_string();
        let idx = dropdown.selected() as usize;
        let ext = formats_clone.get(idx).cloned().unwrap_or_else(|| "tar.gz".to_string());
        let archive_name = format!("{}.{}", stem, ext);

        d.close();

        let dir = Path::new(&files[0])
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let basenames: Vec<String> = files
            .iter()
            .filter_map(|f| {
                Path::new(f)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .collect();

        let cmd = build_compress_command(&ext, &archive_name, &basenames);

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let result = std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&dir)
                .output();
            let msg = match result {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    Err(err.lines().next().unwrap_or("unknown error").to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(msg);
        });

        let p = parent.clone();
        let archive_name_done = archive_name.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    p.announce(
                        &format!("Created {}", archive_name_done),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    p.announce(
                        &format!("Compression failed: {}", e),
                        AccessibleAnnouncementPriority::High,
                    );
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => glib::ControlFlow::Break,
            }
        });
    });

    dlg.present();
    entry_for_focus.grab_focus();
}

fn build_compress_command(ext: &str, archive_name: &str, basenames: &[String]) -> String {
    let quoted_files: String = basenames
        .iter()
        .map(|f| format!("'{}'", f.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let quoted_archive = format!("'{}'", archive_name.replace('\'', "'\\''"));

    match ext {
        "zip" => format!("zip -r {} {}", quoted_archive, quoted_files),
        "tar.gz" => format!("bsdtar czf {} {}", quoted_archive, quoted_files),
        "tar.xz" => format!("bsdtar cJf {} {}", quoted_archive, quoted_files),
        "tar.zst" => format!("bsdtar --zstd -cf {} {}", quoted_archive, quoted_files),
        "tar" => format!("bsdtar cf {} {}", quoted_archive, quoted_files),
        "7z" => format!("7z a {} {}", quoted_archive, quoted_files),
        _ => format!("bsdtar czf {} {}", quoted_archive, quoted_files),
    }
}

fn execute_desktop_action(action: &CustomAction, files: &[String]) {
    let mut cmd_str = action.exec.clone();

    // Substitute %F (multiple files) or %f (single file)
    if cmd_str.contains("%F") {
        let file_args: String = files
            .iter()
            .map(|f| format!("'{}'", f.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        cmd_str = cmd_str.replace("%F", &file_args);
    } else if cmd_str.contains("%f") {
        let file_arg = files
            .first()
            .map(|f| format!("'{}'", f.replace('\'', "'\\''")))
            .unwrap_or_default();
        cmd_str = cmd_str.replace("%f", &file_arg);
    } else if cmd_str.contains("%U") {
        let uri_args: String = files
            .iter()
            .map(|f| format!("'file://{}'", f.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        cmd_str = cmd_str.replace("%U", &uri_args);
    } else if cmd_str.contains("%u") {
        let uri_arg = files
            .first()
            .map(|f| format!("'file://{}'", f.replace('\'', "'\\''")))
            .unwrap_or_default();
        cmd_str = cmd_str.replace("%u", &uri_arg);
    } else {
        // No substitution placeholder -- append files
        for f in files {
            cmd_str.push(' ');
            cmd_str.push_str(&format!("'{}'", f.replace('\'', "'\\''")));
        }
    }

    // Remove other desktop entry field codes
    cmd_str = cmd_str
        .replace("%i", "")
        .replace("%c", "")
        .replace("%k", "");

    std::thread::spawn(move || {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd_str)
            .spawn();
    });
}

fn execute_nautilus_script(action: &CustomAction, files: &[String], current_dir: &str) {
    let paths = files.join("\n");
    let uris: String = files
        .iter()
        .map(|f| format!("file://{}", f))
        .collect::<Vec<_>>()
        .join("\n");
    let current_uri = format!("file://{}", current_dir);

    let exec = action.exec.clone();
    let current_dir = current_dir.to_string();
    std::thread::spawn(move || {
        let _ = std::process::Command::new(&exec)
            .env("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS", &paths)
            .env("NAUTILUS_SCRIPT_SELECTED_URIS", &uris)
            .env("NAUTILUS_SCRIPT_CURRENT_URI", &current_uri)
            .current_dir(&current_dir)
            .spawn();
    });
}
