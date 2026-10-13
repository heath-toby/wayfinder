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
        Some(PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/actions"
        ))),
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

/// Check if a command exists on PATH, or if an absolute/relative path
/// points at an existing file.
fn has_command(cmd: &str) -> bool {
    // Absolute or explicitly relative paths: check existence directly.
    if cmd.starts_with('/') || cmd.starts_with("./") || cmd.starts_with("../") {
        return Path::new(cmd).is_file();
    }
    // Looked up in $PATH here, not by running `which`: this runs as the
    // menu opens, on the main thread, where starting a process can wait on
    // a FUSE file this process has open.
    // Remembered for the run (it's asked each time the menu opens), and
    // folders on FUSE or network mounts in $PATH are skipped: a slow one
    // would hold up the menu.
    static FOUND: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
    if let Some((_, found)) = FOUND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(c, _)| c == cmd)
    {
        return *found;
    }
    use std::os::unix::fs::PermissionsExt;
    let found = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            !crate::rclone_ops::is_fuse_path(&dir.to_string_lossy())
                && std::fs::metadata(dir.join(cmd))
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    });
    FOUND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((cmd.to_string(), found));
    found
}

/// Parse .desktop-like action files from a directory. Also scans immediate
/// subdirectories one level deep, allowing folder-based actions where the
/// script lives alongside its `.desktop` file.
fn load_desktop_actions(dir: &Path) -> Vec<CustomAction> {
    let mut actions = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return actions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // One level of recursion: scan files inside this subdirectory.
            let Ok(sub_entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().is_none_or(|ext| ext != "desktop") {
                    continue;
                }
                if let Some(action) = parse_desktop_action(&sub_path, &path) {
                    actions.push(action);
                }
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "desktop") {
            continue;
        }
        if let Some(action) = parse_desktop_action(&path, dir) {
            actions.push(action);
        }
    }

    actions
}

/// Resolve a command-or-path token relative to `base_dir` if it starts with
/// `./`. Returns the resolved absolute string, or the original unchanged.
fn resolve_relative(token: &str, base_dir: &Path) -> String {
    if let Some(rest) = token.strip_prefix("./") {
        base_dir.join(rest).to_string_lossy().to_string()
    } else {
        token.to_string()
    }
}

/// Parse a single .desktop action file. `base_dir` is the directory
/// containing the `.desktop` file; it is used to resolve `./`-relative
/// paths in `Exec=` and `TryExec=`.
fn parse_desktop_action(path: &Path, base_dir: &Path) -> Option<CustomAction> {
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
            // Resolve a leading ./ in the first token against the
            // action's own directory so folder-based actions can ship
            // their script alongside the .desktop file.
            let resolved = if let Some(rest) = val.strip_prefix("./") {
                // Split off the first whitespace-separated token and resolve it.
                let (first, tail) = match rest.find(char::is_whitespace) {
                    Some(idx) => (&rest[..idx], &rest[idx..]),
                    None => (rest, ""),
                };
                let abs = base_dir.join(first);
                format!("{}{}", abs.to_string_lossy(), tail)
            } else {
                val.to_string()
            };
            exec = Some(resolved);
        } else if let Some(val) = line.strip_prefix("TryExec=") {
            try_exec = Some(resolve_relative(val.trim(), base_dir));
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
/// Announces completion or failure via screen reader on the parent window.
pub fn execute_action(
    action: &CustomAction,
    files: &[String],
    current_dir: &str,
    parent: &gtk::Window,
) {
    let action_name = action.name.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

    if action.is_nautilus_script {
        execute_nautilus_script(action, files, current_dir, tx);
    } else {
        execute_desktop_action(action, files, tx);
    }

    show_action_progress(&action_name, parent, rx);
}

/// Show a progress dialog with a pulsing bar while a background action runs.
/// Closes automatically on completion and announces the result.
fn show_action_progress(
    action_name: &str,
    parent: &gtk::Window,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
) {
    let dlg = gtk::Window::builder()
        .title(format!("{action_name}..."))
        .modal(true)
        .transient_for(parent)
        .default_width(400)
        .resizable(false)
        .build();
    dlg.update_property(&[gtk::accessible::Property::Label(&format!(
        "{action_name} in progress"
    ))]);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let label = gtk::Label::builder()
        .label(format!("{action_name}..."))
        .xalign(0.0)
        .build();

    let progress_bar = gtk::ProgressBar::new();
    progress_bar.set_show_text(true);
    progress_bar.set_text(Some("Working..."));
    progress_bar.update_property(&[gtk::accessible::Property::Label("Progress")]);

    // The action may run for as long as the program it starts (a terminal,
    // an editor): let the user put this window away and carry on — the
    // result is still announced when it's done.
    let hide_btn = gtk::Button::with_label("Continue in background");
    hide_btn.set_halign(gtk::Align::End);
    let d = dlg.clone();
    hide_btn.connect_clicked(move |_| d.close());
    let keys = gtk::EventControllerKey::new();
    let d = dlg.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            d.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    dlg.add_controller(keys);

    vbox.append(&label);
    vbox.append(&progress_bar);
    vbox.append(&hide_btn);
    dlg.set_child(Some(&vbox));
    dlg.present();
    hide_btn.grab_focus();

    let p = parent.clone();
    let name = action_name.to_string();
    let pb = progress_bar;
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        pb.pulse();
        match rx.try_recv() {
            Ok(Ok(())) => {
                dlg.close();
                p.announce(
                    &format!("{name} complete"),
                    AccessibleAnnouncementPriority::Medium,
                );
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                dlg.close();
                p.announce(
                    &format!("{name} failed: {e}"),
                    AccessibleAnnouncementPriority::High,
                );
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => {
                dlg.close();
                glib::ControlFlow::Break
            }
        }
    });
}

/// Show a compress dialog letting the user pick the archive format.
pub fn show_compress_dialog(files: &[String], parent_window: &gtk::Window) {
    if files.is_empty() {
        return;
    }
    // The archive is made in their folder, from their names: they must
    // all be in one (Recent Files can mix folders).
    let folder = |f: &String| Path::new(f).parent().map(Path::to_path_buf);
    if files.iter().any(|f| folder(f) != folder(&files[0])) {
        parent_window.announce(
            "Files from different folders can't be compressed together",
            AccessibleAnnouncementPriority::Medium,
        );
        return;
    }
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
    let first_file = Path::new(&files[0]);
    let default_stem = first_file
        .file_stem()
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
    let format_label = gtk::Label::builder().label("Format:").xalign(0.0).build();

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
    let formats_clone = formats
        .iter()
        .map(|(_, ext)| ext.to_string())
        .collect::<Vec<_>>();
    let parent = parent_window.clone();
    let entry_for_focus = name_entry.clone();
    create_btn.connect_clicked(move |_| {
        let stem = name_entry.text().to_string();
        let idx = dropdown.selected() as usize;
        let ext = formats_clone
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "tar.gz".to_string());
        let archive_name = format!("{stem}.{ext}");

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
        let archive_path = Path::new(&dir).join(&archive_name);
        let existing_name = archive_name.clone();
        std::thread::spawn(move || {
            // zip would add to an existing archive, tar replace it.
            if archive_path.exists() {
                let _ = tx.send(Err(format!("{existing_name} already exists")));
                return;
            }
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

        let progress_label = format!("Compressing {archive_name}");
        show_action_progress(&progress_label, &parent, rx);
    });

    dlg.present();
    entry_for_focus.grab_focus();
}

fn build_compress_command(ext: &str, archive_name: &str, basenames: &[String]) -> String {
    // `./` first, so a name starting with `-` isn't taken for an option.
    let quoted_files: String = basenames
        .iter()
        .map(|f| shell_quote(&format!("./{f}")))
        .collect::<Vec<_>>()
        .join(" ");
    let quoted_archive = shell_quote(&format!("./{archive_name}"));

    match ext {
        "zip" => format!("zip -r {quoted_archive} {quoted_files}"),
        "tar.gz" => format!("bsdtar czf {quoted_archive} {quoted_files}"),
        "tar.xz" => format!("bsdtar cJf {quoted_archive} {quoted_files}"),
        "tar.zst" => format!("bsdtar --zstd -cf {quoted_archive} {quoted_files}"),
        "tar" => format!("bsdtar cf {quoted_archive} {quoted_files}"),
        "7z" => format!("7z a {quoted_archive} {quoted_files}"),
        _ => format!("bsdtar czf {quoted_archive} {quoted_files}"),
    }
}

fn execute_desktop_action(
    action: &CustomAction,
    files: &[String],
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let commands = desktop_commands(&action.exec, files);
    std::thread::spawn(move || {
        // One command per file for %f/%u, one after another: run at once,
        // extractions into the same folder, or password prompts, would
        // collide. Stop at the first failure.
        let mut msg = Ok(());
        for cmd_str in &commands {
            msg = match std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd_str)
                .output()
            {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    Err(err.lines().next().unwrap_or("unknown error").to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            if msg.is_err() {
                break;
            }
        }
        let _ = tx.send(msg);
    });
}

/// Single-quote `s` for `sh`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `file://` URI of a local path, percent-encoded.
fn file_uri(path: &str) -> String {
    gtk::gio::File::for_path(path).uri().to_string()
}

/// The shell commands to run a desktop entry's `Exec=` on `files`, per the
/// desktop-entry spec: `%F`/`%U` take every file at once, `%f`/`%u` one,
/// so the command runs once per file. Other field codes are dropped
/// before files go in, so a `%c` in a file name stays.
fn desktop_commands(exec: &str, files: &[String]) -> Vec<String> {
    let exec = exec.replace("%i", "").replace("%c", "").replace("%k", "");
    let quoted_paths = || files.iter().map(|f| shell_quote(f)).collect::<Vec<_>>();
    let quoted_uris = || {
        files
            .iter()
            .map(|f| shell_quote(&file_uri(f)))
            .collect::<Vec<_>>()
    };
    if exec.contains("%F") {
        vec![exec.replace("%F", &quoted_paths().join(" "))]
    } else if exec.contains("%U") {
        vec![exec.replace("%U", &quoted_uris().join(" "))]
    } else if exec.contains("%f") {
        quoted_paths()
            .iter()
            .map(|f| exec.replace("%f", f))
            .collect()
    } else if exec.contains("%u") {
        quoted_uris()
            .iter()
            .map(|u| exec.replace("%u", u))
            .collect()
    } else {
        // No placeholder: append the files.
        let mut cmd = exec;
        for f in quoted_paths() {
            cmd.push(' ');
            cmd.push_str(&f);
        }
        vec![cmd]
    }
}

fn execute_nautilus_script(
    action: &CustomAction,
    files: &[String],
    current_dir: &str,
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    // In Recent Files (or the Bin) the location isn't a folder: run it in
    // the first file's own.
    let current_dir = if current_dir.contains("://") {
        files
            .first()
            .and_then(|f| Path::new(f).parent())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string())
    } else {
        current_dir.to_string()
    };
    let current_dir = current_dir.as_str();
    let paths = files.join("\n");
    let uris: String = files
        .iter()
        .map(|f| file_uri(f))
        .collect::<Vec<_>>()
        .join("\n");
    let current_uri = file_uri(current_dir);

    let exec = action.exec.clone();
    let current_dir = current_dir.to_string();
    std::thread::spawn(move || {
        let result = std::process::Command::new(&exec)
            .env("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS", &paths)
            .env("NAUTILUS_SCRIPT_SELECTED_URIS", &uris)
            .env("NAUTILUS_SCRIPT_CURRENT_URI", &current_uri)
            .current_dir(&current_dir)
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
}

#[cfg(test)]
mod tests {
    use super::{build_compress_command, desktop_commands};

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_file_codes_run_once_per_file() {
        let cmds = desktop_commands("sha256sum %f", &files(&["/a/x", "/a/y"]));
        assert_eq!(cmds, vec!["sha256sum '/a/x'", "sha256sum '/a/y'"]);
    }

    #[test]
    fn multi_file_codes_take_every_file() {
        let cmds = desktop_commands("tool %F", &files(&["/a/x", "/a/y z"]));
        assert_eq!(cmds, vec!["tool '/a/x' '/a/y z'"]);
    }

    /// `%c` in a file name is part of the name, not a field code.
    #[test]
    fn field_codes_in_file_names_are_kept() {
        let cmds = desktop_commands("open %i %c %f", &files(&["/d/caf%c3%a9.pdf"]));
        assert_eq!(cmds, vec!["open   '/d/caf%c3%a9.pdf'"]);
    }

    #[test]
    fn quotes_in_names_are_escaped() {
        let cmds = desktop_commands("cat %f", &files(&["/d/it's.txt"]));
        assert_eq!(cmds, vec!["cat '/d/it'\\''s.txt'"]);
    }

    #[test]
    fn uris_are_percent_encoded() {
        let cmds = desktop_commands("app %u", &files(&["/d/My File #1.txt"]));
        assert_eq!(cmds, vec!["app 'file:///d/My%20File%20%231.txt'"]);
    }

    /// A name starting with `-` mustn't be read as an option.
    #[test]
    fn compress_names_cant_be_options() {
        let cmd = build_compress_command("zip", "out.zip", &files(&["-T", "a b"]));
        assert_eq!(cmd, "zip -r './out.zip' './-T' './a b'");
    }
}
