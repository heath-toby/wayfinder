//! Offline file caching for slow or unreliable mounts.
//!
//! Users can mark files or folders for offline availability. When marked,
//! Wayfinder copies them to a local cache (`~/.cache/wayfinder/offline/`)
//! and tracks them in a manifest (`~/.config/wayfinder/offline.json`).
//!
//! When a marked path lives on a FUSE mount that becomes unavailable,
//! Wayfinder transparently falls back to the cached copy so the user can
//! still access their files.
//!
//! When the mount returns, pinned files are re-checked and updated from
//! the remote if newer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Persistent file recording every FUSE mountpoint Wayfinder has ever seen
/// for an rclone mount, one path per line. Used to decide when to fall
/// back to the offline cache for a path whose mount has gone away — we
/// only do that for paths inside a *known* mountpoint, never arbitrary
/// directories that happen to share a prefix with the cache layout.
fn known_mountpoints_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wayfinder")
        .join("known_mountpoints")
}

/// Append `mountpoint` to the known-mountpoints file if it isn't already
/// present. Best-effort — failures are silent.
pub fn record_mountpoint(mountpoint: &str) {
    let path = known_mountpoints_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == mountpoint) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut new = existing;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(mountpoint);
    new.push('\n');
    let _ = std::fs::write(&path, new);
}

/// Read the persisted known-mountpoints. Returns an empty vec on missing
/// file or read error.
pub fn known_mountpoints() -> Vec<String> {
    std::fs::read_to_string(known_mountpoints_path())
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether `path` is at-or-inside any known mountpoint. Used to gate cache
/// fallback so it never fires for arbitrary parent directories like
/// `/home/<user>` that merely contain a mountpoint as a child.
pub fn path_is_in_known_mountpoint(path: &str) -> bool {
    known_mountpoints()
        .iter()
        .any(|mp| path == mp || path.starts_with(&format!("{mp}/")))
}

/// A single pinned entry in the manifest.
#[derive(Clone, Debug)]
pub struct PinnedEntry {
    /// The original (remote) path the user pinned.
    pub original_path: String,
    /// True if this is a directory (recursive pin) vs a single file.
    pub is_directory: bool,
    /// Last-known modification time of the remote, in seconds since epoch.
    /// Used to skip re-downloads when the remote hasn't changed.
    pub last_synced_mtime: i64,
    /// Last-known size in bytes — secondary indicator of changes.
    pub last_synced_size: u64,
}

/// Path to the persistent manifest file.
fn manifest_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wayfinder")
        .join("offline.json")
}

/// Path to the cache directory where offline copies are stored.
pub fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wayfinder")
        .join("offline")
}

/// Compute the cache path for a remote path.
///
/// We mirror the directory structure under `cache_root()` so the same
/// remote path always maps to the same local cache path.
pub fn cache_path_for(original_path: &str) -> PathBuf {
    let trimmed = original_path.trim_start_matches('/');
    cache_root().join(trimmed)
}

/// Load the manifest. Returns an empty map if the file doesn't exist or
/// can't be parsed.
pub fn load_manifest() -> HashMap<String, PinnedEntry> {
    let path = manifest_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    parse_manifest(&contents)
}

fn parse_manifest(contents: &str) -> HashMap<String, PinnedEntry> {
    let mut out = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: tab-separated: path<TAB>is_directory<TAB>mtime<TAB>size
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let original_path = parts[0].to_string();
        let is_directory = parts[1] == "1" || parts[1].eq_ignore_ascii_case("true");
        let last_synced_mtime: i64 = parts[2].parse().unwrap_or(0);
        let last_synced_size: u64 = parts[3].parse().unwrap_or(0);
        out.insert(
            original_path.clone(),
            PinnedEntry {
                original_path,
                is_directory,
                last_synced_mtime,
                last_synced_size,
            },
        );
    }
    out
}

/// Save the manifest to disk.
fn save_manifest(manifest: &HashMap<String, PinnedEntry>) -> Result<(), String> {
    let path = manifest_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut contents = String::from("# Wayfinder offline pins — tab-separated: path<TAB>is_directory<TAB>mtime<TAB>size\n");
    let mut entries: Vec<&PinnedEntry> = manifest.values().collect();
    entries.sort_by(|a, b| a.original_path.cmp(&b.original_path));
    for entry in entries {
        contents.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            entry.original_path,
            if entry.is_directory { 1 } else { 0 },
            entry.last_synced_mtime,
            entry.last_synced_size,
        ));
    }
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

/// Check whether a path is currently pinned for offline access.
pub fn is_pinned(path: &str) -> bool {
    let manifest = load_manifest();
    manifest.contains_key(path)
}

/// Record a path in the manifest, assuming the cache copy has already been
/// created (e.g. via `file_ops::copy_with_progress`). Used when the actual
/// file transfer is handled by another component that shows its own progress.
pub fn record_pin(original_path: &str) -> Result<(), String> {
    let src = Path::new(original_path);
    let meta = std::fs::metadata(src).map_err(|e| format!("Cannot read source: {e}"))?;
    let is_directory = meta.is_dir();
    let mtime = mtime_secs(&meta);
    let size = if is_directory { 0 } else { meta.len() };

    let mut manifest = load_manifest();
    manifest.insert(
        original_path.to_string(),
        PinnedEntry {
            original_path: original_path.to_string(),
            is_directory,
            last_synced_mtime: mtime,
            last_synced_size: size,
        },
    );
    save_manifest(&manifest)?;
    Ok(())
}

/// Unpin a path: remove from manifest and delete its cached copy.
pub fn unpin_path(original_path: &str) -> Result<(), String> {
    let mut manifest = load_manifest();
    if manifest.remove(original_path).is_none() {
        return Ok(()); // wasn't pinned
    }
    save_manifest(&manifest)?;
    let cached = cache_path_for(original_path);
    if cached.is_dir() {
        let _ = std::fs::remove_dir_all(&cached);
    } else if cached.is_file() {
        let _ = std::fs::remove_file(&cached);
    }
    Ok(())
}

/// Refresh a single pinned entry if the remote has changed.
/// Returns true if the cached copy was updated.
pub fn refresh_pin(entry: &PinnedEntry) -> Result<bool, String> {
    let src = Path::new(&entry.original_path);
    let Ok(meta) = std::fs::metadata(src) else {
        // Source unreachable — keep cached copy as-is.
        return Ok(false);
    };
    let mtime = mtime_secs(&meta);
    let size = if meta.is_dir() { 0 } else { meta.len() };

    if mtime == entry.last_synced_mtime
        && size == entry.last_synced_size
        && !entry.is_directory
    {
        return Ok(false);
    }

    // Re-copy
    let dest = cache_path_for(&entry.original_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if entry.is_directory {
        // For directories, do a fresh recursive copy
        let _ = std::fs::remove_dir_all(&dest);
        copy_dir_all(src, &dest)?;
    } else {
        std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
    }

    // Update manifest
    let mut manifest = load_manifest();
    if let Some(e) = manifest.get_mut(&entry.original_path) {
        e.last_synced_mtime = mtime;
        e.last_synced_size = size;
    }
    save_manifest(&manifest)?;
    Ok(true)
}

/// Sync all pinned entries — call on startup or when a mount returns.
/// Returns (updated_count, error_count).
pub fn sync_all() -> (usize, usize) {
    let manifest = load_manifest();
    let mut updated = 0;
    let mut errors = 0;
    for entry in manifest.values() {
        match refresh_pin(entry) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(_) => errors += 1,
        }
    }
    (updated, errors)
}

// -----------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        } else if file_type.is_symlink() {
            // Skip symlinks — copying through them could pull data we don't want
            continue;
        }
    }
    Ok(())
}
