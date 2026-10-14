use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Write `contents` to `path` whole or not at all: to a part file of its
/// own (two saves at once mustn't share one), then renamed into place,
/// creating the folder if need be. A reader never sees it half written.
pub fn write_atomically(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = path.with_extension(format!("{}-{n}.wayfinder-part", std::process::id()));
    let result = fs::write(&part, contents).and_then(|()| fs::rename(&part, path));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn config_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wayfinder");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn state_file() -> PathBuf {
    config_dir().join("state")
}

fn load_state() -> HashMap<String, String> {
    let file = state_file();
    match fs::read_to_string(&file) {
        Ok(contents) => {
            let mut map = HashMap::new();
            for line in contents.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    map.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
            map
        }
        Err(_) => HashMap::new(),
    }
}

fn save_state(map: &HashMap<String, String>) {
    let file = state_file();
    let contents: String = map
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = fs::write(&file, contents) {
        log::warn!("Failed to save state: {e}");
    }
}

fn set_value(key: &str, value: &str) {
    let mut map = load_state();
    map.insert(key.to_string(), value.to_string());
    save_state(&map);
}

fn get_value(key: &str) -> Option<String> {
    load_state().get(key).cloned()
}

// -- Public API --

pub fn save_last_directory(path: &str) {
    set_value("directory", path);
}

/// The last directory as saved. It isn't checked here — that could block
/// on a slow mount; navigating to it falls back to the nearest existing
/// parent if it's gone.
pub fn load_last_directory() -> Option<PathBuf> {
    let path_str = get_value("directory")?;
    Some(PathBuf::from(path_str.trim()))
}

pub fn save_view_mode(mode: &str) {
    set_value("view_mode", mode);
}

pub fn load_view_mode() -> Option<String> {
    get_value("view_mode")
}

pub fn save_show_hidden(show: bool) {
    set_value("show_hidden", if show { "true" } else { "false" });
}

pub fn load_show_hidden() -> bool {
    get_value("show_hidden")
        .map(|v| v == "true")
        .unwrap_or(false)
}

pub fn save_sidebar_visible(visible: bool) {
    set_value("sidebar_visible", if visible { "true" } else { "false" });
}

pub fn load_sidebar_visible() -> bool {
    get_value("sidebar_visible")
        .map(|v| v == "true")
        .unwrap_or(true) // default: visible
}

pub fn save_window_size(width: i32, height: i32) {
    let mut map = load_state();
    map.insert("window_width".to_string(), width.to_string());
    map.insert("window_height".to_string(), height.to_string());
    save_state(&map);
}

pub fn load_window_size() -> (i32, i32) {
    let w = get_value("window_width")
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let h = get_value("window_height")
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    (w, h)
}

pub fn save_sort_state(column: u32, ascending: bool) {
    let mut map = load_state();
    map.insert("sort_column".to_string(), column.to_string());
    map.insert(
        "sort_ascending".to_string(),
        if ascending { "true" } else { "false" }.to_string(),
    );
    save_state(&map);
}

pub fn load_sort_state() -> (u32, bool) {
    let col = get_value("sort_column")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let asc = get_value("sort_ascending")
        .map(|v| v == "true")
        .unwrap_or(true);
    (col, asc)
}

pub fn save_zoom_level(level: i32) {
    set_value("zoom_level", &level.to_string());
}

pub fn load_zoom_level() -> i32 {
    get_value("zoom_level")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
}

pub fn save_breadcrumb_visible(visible: bool) {
    set_value("breadcrumb_visible", if visible { "true" } else { "false" });
}

pub fn load_breadcrumb_visible() -> bool {
    get_value("breadcrumb_visible")
        .map(|v| v == "true")
        .unwrap_or(true) // default: visible
}

/// Whether every window of the last session is opened again at startup
/// (on unless turned off).
pub fn save_reopen_windows(reopen: bool) {
    set_value("reopen_windows", if reopen { "true" } else { "false" });
}

pub fn load_reopen_windows() -> bool {
    get_value("reopen_windows")
        .map(|v| v == "true")
        .unwrap_or(true)
}

// -- Window session (multi-window restore) --
// Stored in ~/.config/wayfinder/windows as one directory path per line.

fn windows_path() -> PathBuf {
    config_dir().join("windows")
}

/// Save the list of open window directories for session restore.
pub fn save_window_session(dirs: &[String]) {
    let file = windows_path();
    let contents = dirs.join("\n");
    if let Err(e) = fs::write(&file, contents) {
        log::warn!("Failed to save window session: {e}");
    }
}

/// Load saved window directories. Returns empty vec if none saved.
pub fn load_window_session() -> Vec<PathBuf> {
    let file = windows_path();
    match fs::read_to_string(&file) {
        Ok(contents) => contents
            .lines()
            .filter(|l| !l.trim().is_empty())
            // Not checked here (see load_last_directory).
            .map(|l| PathBuf::from(l.trim()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Clear the saved window session (e.g. after restoring).
pub fn clear_window_session() {
    let file = windows_path();
    let _ = fs::remove_file(&file);
}

// -- Per-file app associations --
// Stored in ~/.config/wayfinder/file_apps as path=desktop_id lines

fn file_apps_path() -> PathBuf {
    config_dir().join("file_apps")
}

fn load_file_apps() -> HashMap<String, String> {
    let file = file_apps_path();
    match fs::read_to_string(&file) {
        Ok(contents) => {
            let mut map = HashMap::new();
            for line in contents.lines() {
                if let Some((key, value)) = parse_file_app(line) {
                    map.insert(key, value);
                }
            }
            map
        }
        Err(_) => HashMap::new(),
    }
}

/// One `path=desktop_id` line. Split at the last `=`: a path may contain
/// one, a desktop id doesn't. The path is kept exactly (it may begin or
/// end with a space).
fn parse_file_app(line: &str) -> Option<(String, String)> {
    let (key, value) = line.rsplit_once('=')?;
    let value = value.trim();
    (!key.is_empty() && !value.is_empty()).then(|| (key.to_string(), value.to_string()))
}

fn save_file_apps(map: &HashMap<String, String>) {
    let file = file_apps_path();
    let contents: String = map
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = fs::write(&file, contents) {
        log::warn!("Failed to save file app associations: {e}");
    }
}

/// Set the preferred app for a specific file path
pub fn save_file_app(file_path: &str, desktop_id: &str) {
    let mut map = load_file_apps();
    map.insert(file_path.to_string(), desktop_id.to_string());
    save_file_apps(&map);
}

/// Get the preferred app desktop ID for a specific file path
pub fn load_file_app(file_path: &str) -> Option<String> {
    load_file_apps().get(file_path).cloned()
}

// -- Sidebar configuration --
// Stored in ~/.config/wayfinder/sidebar as lines of "id:visible"
// where id is the place name and visible is "true" or "false".
// Order of lines determines display order.

fn sidebar_config_path() -> PathBuf {
    config_dir().join("sidebar")
}

/// A single sidebar entry's config: id, visible flag.
#[derive(Clone, Debug)]
pub struct SidebarEntry {
    pub id: String,
    pub visible: bool,
}

pub fn load_sidebar_config() -> Option<Vec<SidebarEntry>> {
    let file = sidebar_config_path();
    let contents = fs::read_to_string(&file).ok()?;
    let entries: Vec<SidebarEntry> = contents
        .lines()
        .filter_map(|line| {
            let (id, vis) = line.split_once(':')?;
            Some(SidebarEntry {
                id: id.trim().to_string(),
                visible: vis.trim() == "true",
            })
        })
        .collect();
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

pub fn save_sidebar_config(entries: &[SidebarEntry]) {
    let file = sidebar_config_path();
    let contents: String = entries
        .iter()
        .map(|e| format!("{}:{}", e.id, if e.visible { "true" } else { "false" }))
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = fs::write(&file, contents) {
        log::warn!("Failed to save sidebar config: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::parse_file_app;

    #[test]
    fn paths_with_equals_signs_keep_their_app() {
        assert_eq!(
            parse_file_app("/home/u/Downloads/watch?v=abc.mp4=vlc.desktop"),
            Some((
                "/home/u/Downloads/watch?v=abc.mp4".into(),
                "vlc.desktop".into()
            ))
        );
        assert_eq!(parse_file_app("no equals sign"), None);
    }
}
