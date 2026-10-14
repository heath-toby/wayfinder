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
    // (Not on a mount, through a link either: looking would hold up the
    // menu; one there is taken to be there.)
    let on_mount = |p: &Path| {
        crate::rclone_ops::is_fuse_path(
            &crate::rclone_ops::resolve_links_outside_mounts(p).to_string_lossy(),
        )
    };
    if cmd.starts_with('/') || cmd.starts_with("./") || cmd.starts_with("../") {
        return on_mount(Path::new(cmd)) || Path::new(cmd).is_file();
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
            !on_mount(&dir)
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
            name = Some(unescape_value(val));
        } else if let Some(val) = line.strip_prefix("Exec=") {
            // As a key file's value is read (`\\` a backslash, `\s` a
            // space…), before it's split into arguments.
            let val = &unescape_value(val);
            // Resolve a leading ./ in the first token against the
            // action's own directory so folder-based actions can ship
            // their script alongside the .desktop file.
            let resolved = if let Some(rest) = val.strip_prefix("./") {
                // Split off the first whitespace-separated token and resolve it.
                let (first, tail) = match rest.find(char::is_whitespace) {
                    Some(idx) => (&rest[..idx], &rest[idx..]),
                    None => (rest, ""),
                };
                // (Quoted: the folder may have a space in its name.)
                let abs = base_dir.join(first);
                format!("{}{}", exec_quote(&abs.to_string_lossy()), tail)
            } else {
                val.to_string()
            };
            exec = Some(resolved);
        } else if let Some(val) = line.strip_prefix("TryExec=") {
            try_exec = Some(resolve_relative(unescape_value(val).trim(), base_dir));
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
    // `kinds`: whether a type that's a kind of `pattern` counts too.
    let matches = |pattern: &str, kinds: bool| {
        // Any type, as file-manager actions write it; all/allfiles is
        // any file, not a folder.
        if matches!(pattern, "*" | "*/*" | "all/all") {
            return true;
        }
        if pattern == "all/allfiles" {
            return mime_type != "inode/directory";
        }
        if pattern == mime_type {
            return true;
        }
        // A kind of it (text/x-python is a text/plain) — but not by the
        // rule that makes everything an application/octet-stream.
        if kinds
            && pattern != "application/octet-stream"
            && gtk::gio::content_type_is_a(mime_type, pattern)
        {
            return true;
        }
        // Support wildcard like "application/*"
        if let Some(prefix) = pattern.strip_suffix("/*") {
            return mime_type.starts_with(&format!("{prefix}/"));
        }
        false
    };
    // `!type`: never for that type, or those `!image/*` names — not for
    // every kind of it, or `!text/plain` would rule out all text.
    if action
        .mime_types
        .iter()
        .any(|p| p.strip_prefix('!').is_some_and(|p| matches(p, false)))
    {
        return false;
    }
    let positive: Vec<&String> = action
        .mime_types
        .iter()
        .filter(|p| !p.starts_with('!'))
        .collect();
    positive.is_empty() || positive.iter().any(|p| matches(p, true))
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
    if action.is_nautilus_script {
        execute_nautilus_script(action, files, &current_dir, tx);
    } else {
        execute_desktop_action(action, files, &current_dir, tx);
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
    hide_btn.update_property(&[gtk::accessible::Property::Label(
        "Continue in background (the result is announced when done)",
    )]);
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

    // Escape closes it, as Close (Cancel) does.
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
    // bsdtar compresses by itself; GNU tar (`tar`) needs the compressor.
    let bsdtar = has_command("bsdtar");
    let tar = bsdtar || has_command("tar");
    if bsdtar || (tar && has_command("gzip")) {
        formats.push(("tar.gz", "tar.gz"));
    }
    if bsdtar || (tar && has_command("xz")) {
        formats.push(("tar.xz", "tar.xz"));
    }
    if bsdtar || (tar && has_command("zstd")) {
        formats.push(("tar.zst", "tar.zst"));
    }
    if tar {
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
    cancel_btn.update_property(&[gtk::accessible::Property::Label("Cancel")]);
    let create_btn = gtk::Button::with_label("Create");
    create_btn.update_property(&[gtk::accessible::Property::Label("Create archive")]);
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
        let stem = name_entry.text().trim().to_string();
        // A name, in the folder of the files — not a hidden `.zip`, nor a
        // path elsewhere.
        if stem.is_empty() || stem.contains('/') || stem == "." || stem == ".." {
            d.announce(
                "Give the archive a name, without / in it",
                AccessibleAnnouncementPriority::High,
            );
            name_entry.grab_focus();
            return;
        }
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

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let archive_path = Path::new(&dir).join(&archive_name);
        let existing_name = archive_name.clone();
        let (ext, archive) = (ext.clone(), archive_name.clone());
        std::thread::spawn(move || {
            // zip would add to an existing archive, tar replace it. (A link
            // by that name too, even one leading nowhere: they'd write
            // through it, elsewhere.)
            if std::fs::symlink_metadata(&archive_path).is_ok() {
                let _ = tx.send(Err(format!("{existing_name} already exists")));
                return;
            }
            // (Asking tar what it is starts it: not on the main thread.)
            let (tar, gnu) = if bsdtar {
                ("bsdtar", false)
            } else {
                ("tar", is_gnu_tar())
            };
            let cmd = build_compress_command(&ext, &archive, &basenames, tar, gnu);
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

/// Whether `tar` is GNU tar (whose exit status 1 is a warning; busybox's
/// and bsdtar's is a failure). Asked once.
fn is_gnu_tar() -> bool {
    static GNU: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *GNU.get_or_init(|| {
        std::process::Command::new("tar")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("GNU tar"))
    })
}

/// `tar`: the tar to run (`bsdtar`, or GNU `tar`: both take these options);
/// `gnu`: whether it's GNU tar (its exit status 1 is a warning).
fn build_compress_command(
    ext: &str,
    archive_name: &str,
    basenames: &[String],
    tar: &str,
    gnu: bool,
) -> String {
    // `./` first, so a name starting with `-` isn't taken for an option.
    let quoted_files: String = basenames
        .iter()
        .map(|f| shell_quote(&format!("./{f}")))
        .collect::<Vec<_>>()
        .join(" ");
    let quoted_archive = shell_quote(&format!("./{archive_name}"));

    match ext {
        // (-y: links kept as links, as tar keeps them — not the folder one
        // leads to, which may be big, or on a mount.)
        "zip" => format!("zip -r -y {quoted_archive} {quoted_files}"),
        "7z" => format!("7z a {quoted_archive} {quoted_files}"),
        _ => {
            let options = match ext {
                "tar.xz" => "cJf",
                "tar.zst" => "--zstd -cf",
                "tar" => "cf",
                _ => "czf",
            };
            let cmd = format!("{tar} {options} {quoted_archive} {quoted_files}");
            // GNU tar's 1 is a warning (a file changed as it was read): the
            // archive is made.
            if gnu {
                format!("{cmd} || [ $? -eq 1 ]")
            } else {
                cmd
            }
        }
    }
}

/// Runs in the folder shown (`current_dir`), as file managers do: an
/// `unzip %f` extracts next to the archive, not wherever Wayfinder started.
fn execute_desktop_action(
    action: &CustomAction,
    files: &[String],
    current_dir: &str,
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let commands = desktop_commands(&action.exec, files);
    let candidates = work_dir_candidates(current_dir, files);
    std::thread::spawn(move || {
        let work_dir = existing_dir(&candidates);
        // One command per file for %f/%u, one after another: run at once,
        // extractions into the same folder, or password prompts, would
        // collide. Stop at the first failure.
        let mut msg = Ok(());
        for argv in &commands {
            let Some((program, args)) = argv.split_first() else {
                msg = Err("the action's Exec is empty".to_string());
                break;
            };
            msg = match std::process::Command::new(program)
                .args(args)
                .current_dir(&work_dir)
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

/// Where an action may run, best first: the folder shown, the first
/// file's folder, home (the folder shown may have been deleted meanwhile).
fn work_dir_candidates(current_dir: &str, files: &[String]) -> Vec<std::path::PathBuf> {
    let mut out = vec![std::path::PathBuf::from(current_dir)];
    out.extend(
        files
            .first()
            .and_then(|f| Path::new(f).parent())
            .map(Path::to_path_buf),
    );
    out.extend(dirs::home_dir());
    out
}

/// The first of `candidates` that's a folder (checked off the main thread:
/// it may be on a slow mount); `/` if none is.
fn existing_dir(candidates: &[std::path::PathBuf]) -> std::path::PathBuf {
    candidates
        .iter()
        .find(|d| d.is_dir())
        .cloned()
        .unwrap_or_else(|| "/".into())
}

/// Single-quote `s` for `sh`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `file://` URI of a local path, percent-encoded.
fn file_uri(path: &str) -> String {
    gtk::gio::File::for_path(path).uri().to_string()
}

/// A key file's string value, its escapes read: `\s` space, `\n` `\t` `\r`,
/// `\\` a backslash (any other `\` kept, for `split_exec`).
fn unescape_value(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            _ => {
                out.push('\\');
                continue;
            }
        }
        chars.next();
    }
    out
}

/// `Exec=` split into its arguments, as GLib reads it: separated by
/// spaces; in double quotes, spaces are kept and `\` escapes `"`, `` ` ``,
/// `$` and `\`; in single quotes everything is kept as it is (so an
/// `sh -c '…'` script is one argument); outside quotes `\` escapes the
/// next character.
#[cfg(test)]
fn split_exec(exec: &str) -> Vec<String> {
    split_exec_quoted(exec)
        .into_iter()
        .map(|(arg, _)| arg)
        .collect()
}

/// `split_exec`, each argument with whether any of it was quoted.
fn split_exec_quoted(exec: &str) -> Vec<(String, bool)> {
    let mut args = Vec::new();
    let mut arg = String::new();
    let mut in_arg = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_arg = true;
                quoted = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some('"' | '`' | '$' | '\\') => arg.push(chars.next().unwrap()),
                            _ => arg.push('\\'),
                        },
                        c => arg.push(c),
                    }
                }
            }
            '\'' => {
                in_arg = true;
                quoted = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    arg.push(c);
                }
            }
            '\\' => {
                // (An escaped space or the like: not quoting what's around.)
                in_arg = true;
                if let Some(c) = chars.next() {
                    arg.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_arg {
                    args.push((std::mem::take(&mut arg), quoted));
                    in_arg = false;
                    quoted = false;
                }
            }
            c => {
                in_arg = true;
                arg.push(c);
            }
        }
    }
    if in_arg {
        args.push((arg, quoted));
    }
    args
}

/// `s` as one `Exec=` argument (quoted, as `split_exec` reads it).
fn exec_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The commands (each its arguments) to run a desktop entry's `Exec=` on
/// `files`, per the desktop-entry spec: `%F`/`%U` (an argument of their
/// own) take every file at once, `%f`/`%u` one, so the command runs once
/// per file. Field codes are replaced in each argument after it's split
/// out, and the command is run directly, not by a shell: a file's name is
/// only ever data (one with `$(…)` in it runs nothing), quoted or not.
/// Other field codes are dropped; a `%c` in a file name stays.
fn desktop_commands(exec: &str, files: &[String]) -> Vec<Vec<String>> {
    let args = split_exec_quoted(exec);
    // Field codes are filled in unquoted arguments only (anywhere in one:
    // `%f`, `%f~`, `--input=%f`). Quoted text is left as written — the spec
    // leaves codes there undefined, and it's where inline code goes
    // (`sh -c '…'`, `python3 -c '…'`): a file's name put in code would be
    // run (`$(…)` in one); such an action gets the files after it instead
    // (a script has them as "$1"). (An unquoted code where an interpreter
    // takes its code — `sh -c %f` — is the action's own doing, as in GLib.)
    // (Quoted, only a code on its own: `"%f"` is one argument either way.)
    let fillable = |i: usize| {
        let (arg, quoted) = &args[i];
        !quoted || matches!(arg.as_str(), "%f" | "%u" | "%F" | "%U")
    };
    // One argument with its codes filled: `%f` the file, `%u` its URI,
    // `%%` a `%`; `%i %c %k` and the deprecated `%d %D %n %N %v %m` go.
    let fill = |arg: &str, file: &str, uri: &str| -> String {
        let mut out = String::new();
        let mut chars = arg.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('f') => out.push_str(file),
                Some('u') => out.push_str(uri),
                Some('%') => out.push('%'),
                Some('i' | 'c' | 'k' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        out
    };
    let has_code = |i: usize, codes: &[&str]| {
        fillable(i)
            && codes
                .iter()
                .any(|c| args[i].0.replace("%%", "").contains(c))
    };
    let many = (0..args.len()).any(|i| fillable(i) && (args[i].0 == "%F" || args[i].0 == "%U"));
    let one = (0..args.len()).any(|i| has_code(i, &["%f", "%u"]));
    let uris: Vec<String> = files.iter().map(|f| file_uri(f)).collect();
    // Filled as given, an argument that was only a dropped code (`%i`) goes.
    let argument = |i: usize, file: &str, uri: &str| -> Option<String> {
        let (arg, _) = &args[i];
        if !fillable(i) {
            return Some(arg.clone());
        }
        let filled = fill(arg, file, uri);
        (!filled.is_empty() || arg.is_empty()).then_some(filled)
    };
    if many {
        let mut argv = Vec::new();
        for (i, (arg, _)) in args.iter().enumerate() {
            match arg.as_str() {
                "%F" if fillable(i) => argv.extend(files.iter().cloned()),
                "%U" if fillable(i) => argv.extend(uris.iter().cloned()),
                // (A single-file code beside them is kept as written: an
                // argument dropped would shift the rest — a file taking an
                // output's place.)
                _ if has_code(i, &["%f", "%u"]) => argv.push(arg.clone()),
                _ => argv.extend(argument(i, "", "")),
            }
        }
        vec![argv]
    } else if one {
        files
            .iter()
            .zip(&uris)
            .map(|(f, u)| (0..args.len()).filter_map(|i| argument(i, f, u)).collect())
            .collect()
    } else {
        // No placeholder (or only where it isn't filled): the files after it.
        let mut argv: Vec<String> = (0..args.len())
            .filter_map(|i| argument(i, "", ""))
            .collect();
        argv.extend(files.iter().cloned());
        vec![argv]
    }
}

fn execute_nautilus_script(
    action: &CustomAction,
    files: &[String],
    current_dir: &str,
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    // Each followed by a newline, as Nautilus gives them (a script's
    // `while read` loop sees the last one too).
    let paths: String = files.iter().map(|f| format!("{f}\n")).collect();
    let uris: String = files.iter().map(|f| format!("{}\n", file_uri(f))).collect();
    let current_uri = file_uri(current_dir);

    let exec = action.exec.clone();
    let candidates = work_dir_candidates(current_dir, files);
    let args = files.to_vec();
    std::thread::spawn(move || {
        let current_dir = existing_dir(&candidates);
        // The files as arguments too, as Nautilus passes them ("$@").
        let result = std::process::Command::new(&exec)
            .args(&args)
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
    use super::{
        build_compress_command, desktop_commands, exec_quote, matches_mime, split_exec,
        CustomAction,
    };

    #[test]
    fn tar_archives_with_either_tar() {
        let names = files(&["a"]);
        assert_eq!(
            build_compress_command("tar.zst", "x.tar.zst", &names, "bsdtar", false),
            "bsdtar --zstd -cf './x.tar.zst' './a'"
        );
        // GNU tar: a warning (1) isn't a failure. (Any other tar's 1 is.)
        assert_eq!(
            build_compress_command("tar.xz", "x.tar.xz", &names, "tar", true),
            "tar cJf './x.tar.xz' './a' || [ $? -eq 1 ]"
        );
        assert_eq!(
            build_compress_command("tar.xz", "x.tar.xz", &names, "tar", false),
            "tar cJf './x.tar.xz' './a'"
        );
    }

    #[test]
    fn mime_patterns_as_file_manager_actions_write_them() {
        let action = |types: &[&str]| CustomAction {
            name: "a".into(),
            exec: "true".into(),
            mime_types: types.iter().map(|t| t.to_string()).collect(),
            is_nautilus_script: false,
        };
        assert!(matches_mime(&action(&["*/*"]), "text/plain"));
        assert!(matches_mime(&action(&["all/allfiles"]), "image/png"));
        assert!(!matches_mime(&action(&["all/allfiles"]), "inode/directory"));
        assert!(matches_mime(&action(&["image/*"]), "image/png"));
        assert!(!matches_mime(&action(&["image/*"]), "text/plain"));
        assert!(!matches_mime(
            &action(&["*", "!inode/directory"]),
            "inode/directory"
        ));
        assert!(matches_mime(&action(&["!inode/directory"]), "text/plain"));
        assert!(matches_mime(&action(&["text/plain"]), "text/x-python"));
        assert!(!matches_mime(
            &action(&["application/octet-stream"]),
            "image/png"
        ));
        assert!(matches_mime(&action(&["*", "!text/plain"]), "text/html"));
        assert!(!matches_mime(&action(&["*", "!image/*"]), "image/png"));
    }

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_file_codes_run_once_per_file() {
        let cmds = desktop_commands("sha256sum %f", &files(&["/a/x", "/a/y"]));
        assert_eq!(
            cmds,
            vec![argv(&["sha256sum", "/a/x"]), argv(&["sha256sum", "/a/y"])]
        );
    }

    #[test]
    fn multi_file_codes_take_every_file() {
        let cmds = desktop_commands("tool %F", &files(&["/a/x", "/a/y z"]));
        assert_eq!(cmds, vec![argv(&["tool", "/a/x", "/a/y z"])]);
    }

    /// `%c` in a file name is part of the name, not a field code.
    #[test]
    fn field_codes_in_file_names_are_kept() {
        let cmds = desktop_commands("open %i %c %f", &files(&["/d/caf%c3%a9.pdf"]));
        assert_eq!(cmds, vec![argv(&["open", "/d/caf%c3%a9.pdf"])]);
        let cmds = desktop_commands("gnome-terminal --working-directory=%f", &files(&["/d/a b"]));
        assert_eq!(
            cmds,
            vec![argv(&["gnome-terminal", "--working-directory=/d/a b"])]
        );
    }

    /// Code in any language, in any wrapper: never written into.
    #[test]
    fn no_inline_code_gets_a_file_name() {
        let evil = "/d/$(touch PWNED)\")+__import__(\"os\").system(\"id\")+(\".txt";
        for exec in [
            "bash -lc 'cat \"%f\"'",
            "sh -ec 'cat \"%f\"'",
            "env sh -c 'cat \"%f\"'",
            "foot -e sh -c 'less \"%f\"'",
            "python3 -c 'print(\"%f\")'",
        ] {
            for argv in desktop_commands(exec, &files(&[evil])) {
                let text: Vec<&String> = argv.iter().filter(|a| *a != evil).collect();
                assert!(
                    text.iter().all(|a| !a.contains("PWNED")),
                    "{exec}: {argv:?}"
                );
            }
        }
    }

    /// A file's name is data: quoted in Exec or not, nothing in it runs.
    #[test]
    fn a_files_name_never_runs_anything() {
        let cmds = desktop_commands("xdg-open \"%f\"", &files(&["/d/$(echo INJECTED) it's.txt"]));
        assert_eq!(
            cmds,
            vec![argv(&["xdg-open", "/d/$(echo INJECTED) it's.txt"])]
        );
        let cmds = desktop_commands("sh -c 'x' \"%f\"", &files(&["/d/My File.txt"]));
        assert_eq!(cmds, vec![argv(&["sh", "-c", "x", "/d/My File.txt"])]);
    }

    /// Every bundled action splits into a program and its arguments, the
    /// file among them (or none, for one that takes none).
    #[test]
    fn bundled_actions_split_sensibly() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/actions");
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            let Some(exec) = text.lines().find_map(|l| l.strip_prefix("Exec=")) else {
                continue;
            };
            if exec.starts_with("__") {
                continue;
            }
            let cmds = desktop_commands(exec, &files(&["/d/My $(x) file.tar.gz"]));
            let name = entry.file_name();
            assert!(!cmds.is_empty() && !cmds[0].is_empty(), "{name:?}");
            let argv = &cmds[0];
            assert!(!argv[0].contains(' '), "{name:?}: {argv:?}");
            assert!(
                argv.iter().any(|a| a.contains("/d/My $(x) file.tar.gz")),
                "{name:?}: {argv:?}"
            );
            if argv[0] == "sh" {
                assert_eq!(argv.len(), 5, "{name:?}: {argv:?}");
            }
        }
    }

    /// A file's name never goes into a shell script's text, even where the
    /// script names a field code; `%F` in a script isn't one.
    #[test]
    fn scripts_are_never_written_into() {
        let cmds = desktop_commands("sh -c 'cat \"%f\"'", &files(&["/d/$(touch PWNED).txt"]));
        assert_eq!(
            cmds,
            vec![argv(&["sh", "-c", "cat \"%f\"", "/d/$(touch PWNED).txt"])]
        );
        let exec = "sh -c 'cp \"$1\" \"$1.$(date +%F)\"' _ %f";
        let cmds = desktop_commands(exec, &files(&["/d/a"]));
        assert_eq!(
            cmds,
            vec![argv(&[
                "sh",
                "-c",
                "cp \"$1\" \"$1.$(date +%F)\"",
                "_",
                "/d/a"
            ])]
        );
    }

    #[test]
    fn key_file_escapes_are_read_first() {
        let exec = super::unescape_value("sh -c \"echo \\\\\"$1\\\\\"\" _ %f");
        assert_eq!(
            split_exec(&exec),
            argv(&["sh", "-c", "echo \"$1\"", "_", "%f"])
        );
        // A folder with a % in its name, quoted as the parser quotes it.
        let cmds = desktop_commands(
            &format!("{} %f", exec_quote("/a/50%cool/run.sh")),
            &files(&["/x"]),
        );
        assert_eq!(cmds, vec![argv(&["/a/50%cool/run.sh", "/x"])]);
    }

    /// Unquoted, a code goes anywhere in an argument (a backup's name).
    #[test]
    fn unquoted_codes_are_filled_inside_arguments() {
        let cmds = desktop_commands("rm -f %f~", &files(&["/d/a"]));
        assert_eq!(cmds, vec![argv(&["rm", "-f", "/d/a~"])]);
        let cmds = desktop_commands("mv %f %f.bak", &files(&["/d/a", "/d/b"]));
        assert_eq!(
            cmds,
            vec![
                argv(&["mv", "/d/a", "/d/a.bak"]),
                argv(&["mv", "/d/b", "/d/b.bak"])
            ]
        );
        let cmds = desktop_commands("convert %f -resize 50%% /tmp/o.png", &files(&["/d/a"]));
        assert_eq!(
            cmds,
            vec![argv(&["convert", "/d/a", "-resize", "50%", "/tmp/o.png"])]
        );
        let cmds = desktop_commands("app --icon=%i %f", &files(&["/d/a"]));
        assert_eq!(cmds, vec![argv(&["app", "--icon=", "/d/a"])]);
        // Beside %F, a %f argument stays as written (dropped, the rest would
        // shift: a file in the output's place).
        let cmds = desktop_commands("tar -czf %f.tar.gz %F", &files(&["/d/a", "/d/b"]));
        assert_eq!(
            cmds,
            vec![argv(&["tar", "-czf", "%f.tar.gz", "/d/a", "/d/b"])]
        );
    }

    /// A `-c` that's a script file's option is filled as any; `\ ` in an
    /// unquoted argument doesn't stop its codes being filled.
    #[test]
    fn options_and_escaped_spaces_are_filled() {
        let cmds = desktop_commands("sh ./convert.sh -c %f", &files(&["/x"]));
        assert_eq!(cmds, vec![argv(&["sh", "./convert.sh", "-c", "/x"])]);
        let cmds = desktop_commands("cp %f /mnt/My\\ Backup/%f.bak", &files(&["/x"]));
        assert_eq!(cmds, vec![argv(&["cp", "/x", "/mnt/My Backup//x.bak"])]);
        let cmds = desktop_commands("sh -e %f", &files(&["/d/x.sh"]));
        assert_eq!(cmds, vec![argv(&["sh", "-e", "/d/x.sh"])]);
    }

    /// The bundled actions: an `sh -c '…'` script, `%` in it kept.
    #[test]
    fn script_actions_get_their_script_whole() {
        let exec = "sh -c 'stem=\"${1%.tar.gz}\"; echo \"$stem\"' _ %f";
        let cmds = desktop_commands(exec, &files(&["/d/a b.tar.gz"]));
        assert_eq!(
            cmds,
            vec![argv(&[
                "sh",
                "-c",
                "stem=\"${1%.tar.gz}\"; echo \"$stem\"",
                "_",
                "/d/a b.tar.gz"
            ])]
        );
        let out = std::process::Command::new(&cmds[0][0])
            .args(&cmds[0][1..])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/d/a b\n");
    }

    #[test]
    fn exec_arguments_split_as_the_spec_says() {
        assert_eq!(
            split_exec("app \"two words\" plain \"a\\\"q\" 100%% 'it''s' a\\ b"),
            argv(&["app", "two words", "plain", "a\"q", "100%%", "its", "a b"])
        );
        assert_eq!(
            split_exec(&exec_quote("/x/My Tools/run$.sh")),
            argv(&["/x/My Tools/run$.sh"])
        );
    }

    #[test]
    fn uris_are_percent_encoded() {
        let cmds = desktop_commands("app %u", &files(&["/d/My File #1.txt"]));
        assert_eq!(cmds, vec![argv(&["app", "file:///d/My%20File%20%231.txt"])]);
    }

    /// A name starting with `-` mustn't be read as an option.
    #[test]
    fn compress_names_cant_be_options() {
        let cmd = build_compress_command("zip", "out.zip", &files(&["-T", "a b"]), "tar", false);
        assert_eq!(cmd, "zip -r -y './out.zip' './-T' './a b'");
    }
}
