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

mod sleep;

use std::collections::{HashMap, HashSet};
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
    let mut contents = String::from(
        "# Wayfinder offline pins — tab-separated: path<TAB>is_directory<TAB>mtime<TAB>size\n",
    );
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
    // Only pin what really arrived: the download may have failed, been
    // cancelled or skipped.
    check_downloaded(&meta, &cache_path_for(original_path))?;
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

/// Whether the offline copy at `cached` is complete for a source with
/// metadata `src_meta`: there, of the same kind, and for a file the same
/// size.
fn check_downloaded(src_meta: &std::fs::Metadata, cached: &Path) -> Result<(), String> {
    let Ok(cached_meta) = std::fs::metadata(cached) else {
        return Err("the download didn't complete".to_string());
    };
    if cached_meta.is_dir() != src_meta.is_dir()
        || (src_meta.is_file() && cached_meta.len() != src_meta.len())
    {
        return Err("the downloaded copy is incomplete".to_string());
    }
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
///
/// Directories are refreshed incrementally: only files whose size or
/// mtime differ from the cached copy are downloaded, and each lands in a
/// temporary file first, so an interrupted refresh never loses the
/// existing offline copy.
pub fn refresh_pin(entry: &PinnedEntry) -> Result<bool, String> {
    // Pins live on rclone mounts. If the mount isn't up, the path may still
    // exist as an empty mountpoint directory, and syncing from it would
    // delete the offline copy just when it's needed.
    if !crate::rclone_ops::is_fuse_path(&entry.original_path) {
        return Ok(false);
    }
    let src = Path::new(&entry.original_path);
    let meta = {
        let _io = sleep::io_step();
        std::fs::metadata(src)
    };
    let Ok(meta) = meta else {
        // Source unreachable — keep cached copy as-is.
        return Ok(false);
    };
    let mtime = mtime_secs(&meta);
    let size = if meta.is_dir() { 0 } else { meta.len() };

    if mtime == entry.last_synced_mtime && size == entry.last_synced_size && !entry.is_directory {
        return Ok(false);
    }

    let dest = cache_path_for(&entry.original_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Fetch through rclone straight from the remote when we can (see
    // `fetch`); resolved once per pin, since it runs `ps`.
    let remote = crate::rclone_ops::rclone_for_path(&entry.original_path)
        .filter(|_| crate::rclone_ops::rclone_available())
        .map(|t| t.full_remote_spec);
    let changed = if entry.is_directory {
        sync_dir(src, &dest, remote.as_deref())?
    } else {
        sync_file(src, &meta, &dest, remote.as_deref())?
    };

    // Update manifest
    let mut manifest = load_manifest();
    if let Some(e) = manifest.get_mut(&entry.original_path) {
        e.last_synced_mtime = mtime;
        e.last_synced_size = size;
    }
    save_manifest(&manifest)?;
    Ok(changed)
}

/// Sync all pinned entries — call on startup or when a mount returns.
/// Returns (updated_count, error_count).
pub fn sync_all() -> (usize, usize) {
    let manifest = load_manifest();
    if !manifest.is_empty() {
        sleep::ensure_watching();
    }
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

/// Suffix for a download in progress; renamed into place when complete.
const PART_SUFFIX: &str = ".wayfinder-part";

/// Bring `dst` in line with the directory `src`. Returns true if anything
/// was copied or removed.
fn sync_dir(src: &Path, dst: &Path, remote: Option<&str>) -> Result<bool, String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let entries: Vec<std::fs::DirEntry> = {
        let _io = sleep::io_step();
        std::fs::read_dir(src)
            .and_then(|it| it.collect::<Result<_, _>>())
            .map_err(|e| e.to_string())?
    };
    let mut changed = false;
    let mut present: HashSet<std::ffi::OsString> = HashSet::new();
    for entry in entries {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        present.insert(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let child = remote.map(|r| remote_child(r, &entry.file_name().to_string_lossy()));
            changed |= sync_dir(&from, &to, child.as_deref())?;
        } else if file_type.is_file() {
            let meta = {
                let _io = sleep::io_step();
                std::fs::metadata(&from)
            };
            let meta = meta.map_err(|e| e.to_string())?;
            let child = remote.map(|r| remote_child(r, &entry.file_name().to_string_lossy()));
            changed |= sync_file(&from, &meta, &to, child.as_deref())?;
        }
        // Skip symlinks — copying through them could pull data we don't want
    }

    // Drop cached entries that no longer exist on the remote. The listing
    // above is complete (any error returned early), so this is safe.
    for entry in std::fs::read_dir(dst).map_err(|e| e.to_string())?.flatten() {
        if present.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        changed |= removed.is_ok();
    }
    Ok(changed)
}

/// Copy `src` to `dst` unless the cached copy already matches its size
/// and mtime. Returns true if a copy was made.
fn sync_file(
    src: &Path,
    src_meta: &std::fs::Metadata,
    dst: &Path,
    remote: Option<&str>,
) -> Result<bool, String> {
    if let Ok(dst_meta) = std::fs::metadata(dst) {
        if dst_meta.len() == src_meta.len() && mtime_secs(&dst_meta) == mtime_secs(src_meta) {
            return Ok(false);
        }
    }

    let mut part_name = dst.file_name().unwrap_or_default().to_os_string();
    part_name.push(PART_SUFFIX);
    let part = dst.with_file_name(part_name);
    // The copy keeps the mtime of what was actually fetched (rclone and
    // `cp --preserve` both set it). If the mount shows something newer —
    // say an upload still pending in rclone's write cache — the next
    // refresh sees the difference and fetches again.
    let result = fetch(src, &part, remote).and_then(|()| std::fs::rename(&part, dst));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    Ok(true)
}

/// Download `src` (on the mount) to the local file `dst` in a child process.
///
/// Not in this process: a FUSE file held open here gets a FLUSH request
/// each time any copy of its descriptor is closed — including the copy
/// every newly spawned child closes as it starts. While the daemon is slow
/// that holds up the spawn, and Wayfinder spawns from its main thread
/// (`ps`, apps opening files), so the window would freeze. With rclone the
/// child also skips the mount and fetches straight from the remote; if
/// that fails (e.g. the mount uses a config this rclone doesn't see), the
/// file is copied through the mount instead.
fn fetch(src: &Path, dst: &Path, remote: Option<&str>) -> std::io::Result<()> {
    if let Some(remote) = remote {
        let mut cmd = std::process::Command::new("rclone");
        cmd.arg("copyto").arg(remote).arg(dst);
        if run_quietly(&mut cmd)?.success() {
            return Ok(());
        }
        log::warn!("rclone copyto {remote} failed; copying through the mount");
    }
    let mut cmd = std::process::Command::new("cp");
    cmd.arg("--preserve=timestamps").arg("--").arg(src).arg(dst);
    let status = run_quietly(&mut cmd)?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "copying {} failed ({status})",
            src.display()
        )))
    }
}

fn run_quietly(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sleep::run_child(cmd)
}

/// The remote spec of `name` inside the remote directory `remote`.
fn remote_child(remote: &str, name: &str) -> String {
    if remote.ends_with(':') || remote.ends_with('/') {
        format!("{remote}{name}")
    } else {
        format!("{remote}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::check_downloaded;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wayfinder-pin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_or_partial_copies_are_not_pinned() {
        let d = dir("check");
        let src = d.join("src.bin");
        std::fs::write(&src, b"0123456789").unwrap();
        let meta = std::fs::metadata(&src).unwrap();

        assert!(check_downloaded(&meta, &d.join("missing")).is_err());
        let partial = d.join("partial.bin");
        std::fs::write(&partial, b"01234").unwrap();
        assert!(check_downloaded(&meta, &partial).is_err());
        // rclone `copy` put the file in a folder of its name.
        let nested = d.join("nested.bin");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(check_downloaded(&meta, &nested).is_err());

        let full = d.join("full.bin");
        std::fs::write(&full, b"0123456789").unwrap();
        assert!(check_downloaded(&meta, &full).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
