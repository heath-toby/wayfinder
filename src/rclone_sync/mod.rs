//! rclone bisync integration for true bidirectional folder sync.
//!
//! Users pair a local folder with a remote path (e.g. `Proton:Work`) and
//! Wayfinder runs `rclone bisync` to keep them in sync. The first run uses
//! `--resync` to establish a baseline; subsequent runs are normal bisync.
//!
//! Sync pairs are stored in `~/.config/wayfinder/sync_pairs` as a simple
//! tab-separated text file for resilience.

pub mod dialog;

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SyncPair {
    /// User-facing path — what the user right-clicked. For pairs created
    /// from inside an rclone FUSE mount, this is the FUSE path; otherwise
    /// it's the same as `sync_local_path`.
    pub local_path: String,
    /// Path passed to `rclone bisync` as the local arg. For rclone FUSE
    /// mounts this is the offline cache path so bisync isn't pointing the
    /// remote at itself; for plain pairs it equals `local_path`.
    pub sync_local_path: String,
    /// Rclone remote spec, e.g. "Proton:Work".
    pub remote: String,
    /// Last successful sync time (seconds since epoch). 0 if never synced.
    pub last_sync_time: i64,
    /// True once the initial `--resync` baseline has been established.
    pub initial_resync_done: bool,
}

/// Path to the persistent sync-pairs file.
fn pairs_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wayfinder")
        .join("sync_pairs")
}

/// Load all configured sync pairs.
pub fn load_pairs() -> HashMap<String, SyncPair> {
    let Ok(contents) = std::fs::read_to_string(pairs_path()) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // tab-separated. Legacy 4-field rows: local<TAB>remote<TAB>last_sync<TAB>resync_done
        // New 5-field rows: local<TAB>remote<TAB>last_sync<TAB>resync_done<TAB>sync_local_path
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let local = parts[0].to_string();
        let remote = parts[1].to_string();
        let last_sync_time: i64 = parts[2].parse().unwrap_or(0);
        let initial_resync_done = parts[3] == "1" || parts[3].eq_ignore_ascii_case("true");
        let sync_local_path = if parts.len() >= 5 {
            parts[4].to_string()
        } else {
            local.clone()
        };
        out.insert(
            local.clone(),
            SyncPair {
                local_path: local,
                sync_local_path,
                remote,
                last_sync_time,
                initial_resync_done,
            },
        );
    }
    out
}

/// Save all pairs back to disk.
fn save_pairs(pairs: &HashMap<String, SyncPair>) -> Result<(), String> {
    let path = pairs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut contents = String::from(
        "# Wayfinder rclone sync pairs — local<TAB>remote<TAB>last_sync_time<TAB>resync_done<TAB>sync_local_path\n",
    );
    let mut entries: Vec<&SyncPair> = pairs.values().collect();
    entries.sort_by(|a, b| a.local_path.cmp(&b.local_path));
    for entry in entries {
        contents.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            entry.local_path,
            entry.remote,
            entry.last_sync_time,
            if entry.initial_resync_done { 1 } else { 0 },
            entry.sync_local_path,
        ));
    }
    // Whole or not at all: a torn file would read as no pairs, and the
    // offline refresh would then treat sync's folders as its own.
    crate::state::write_atomically(&path, contents.as_bytes()).map_err(|e| e.to_string())
}

/// Look up the configured sync pair for a given local path, if any.
pub fn pair_for(local_path: &str) -> Option<SyncPair> {
    load_pairs().get(local_path).cloned()
}

/// Why pairing a folder whose sync side is `sync_local` with `remote` is
/// refused, if it is. A mount's folder syncs through its offline copy,
/// which holds the files of the remote it's mounted from (`mount_remote`)
/// and of the one it was paired with (`old_remote`): synced with any other,
/// they'd be merged into it. (Compared as written, as the sync's own record
/// is: another spelling would start the sync afresh.)
fn remote_refusal(
    sync_local: &std::path::Path,
    remote: &str,
    mount_remote: Option<&str>,
    old_remote: Option<&str>,
    cache_root: &std::path::Path,
) -> Option<String> {
    if !sync_local.starts_with(cache_root) {
        return None;
    }
    if let Some(m) = mount_remote.filter(|m| *m != remote) {
        return Some(format!(
            "this folder is on a drive mounted from {m}: it can only be synced with that"
        ));
    }
    if let Some(old) = old_remote.filter(|old| *old != remote) {
        return Some(format!(
            "this folder is synced with {old}, and its offline copy holds that remote's files: it can't be synced with another"
        ));
    }
    None
}

/// Add or update a sync pair. Does not run a sync — the user must trigger
/// that explicitly so they can review the dialog first.
///
/// `local_path` is the user-facing key (what they right-clicked).
/// `sync_local_path` is the directory passed to bisync as the local arg —
/// for rclone FUSE mounts this should be the offline cache path.
pub fn set_pair(local_path: &str, sync_local_path: &str, remote: &str) -> Result<(), String> {
    let mut pairs = load_pairs();
    // A mount's folder edited while its drive is down: its sync side is
    // still the offline copy, not the bare (empty) mountpoint.
    let cache_root = crate::offline::cache_root();
    let sync_local_path = match pairs.get(local_path) {
        Some(old)
            if std::path::Path::new(&old.sync_local_path).starts_with(&cache_root)
                && !std::path::Path::new(sync_local_path).starts_with(&cache_root)
                && crate::offline::path_is_in_known_mountpoint(local_path) =>
        {
            old.sync_local_path.clone()
        }
        _ => sync_local_path.to_string(),
    };
    let sync_local_path = sync_local_path.as_str();
    let mount_remote = crate::rclone_ops::rclone_for_path(
        &crate::rclone_ops::resolve_links_outside_mounts(std::path::Path::new(local_path))
            .to_string_lossy(),
    )
    .map(|t| t.full_remote_spec);
    if let Some(why) = remote_refusal(
        std::path::Path::new(sync_local_path),
        remote,
        mount_remote.as_deref(),
        pairs.get(local_path).map(|p| p.remote.as_str()),
        &cache_root,
    ) {
        return Err(why);
    }
    // A pair whose ends changed is a new pair: its first sync must
    // establish a fresh baseline (--resync), and it hasn't synced yet.
    let existing = pairs
        .get(local_path)
        .filter(|p| p.remote == remote && p.sync_local_path == sync_local_path)
        .cloned();
    pairs.insert(
        local_path.to_string(),
        SyncPair {
            local_path: local_path.to_string(),
            sync_local_path: sync_local_path.to_string(),
            remote: remote.to_string(),
            last_sync_time: existing.as_ref().map(|p| p.last_sync_time).unwrap_or(0),
            initial_resync_done: existing
                .as_ref()
                .map(|p| p.initial_resync_done)
                .unwrap_or(false),
        },
    );
    save_pairs(&pairs)
}

/// Remove a sync pair from the manifest. Does not delete any files.
pub fn remove_pair(local_path: &str) -> Result<(), String> {
    let mut pairs = load_pairs();
    if pairs.remove(local_path).is_none() {
        return Ok(());
    }
    save_pairs(&pairs)
}

/// Mark the initial resync as complete and update the last-sync time.
fn mark_synced(local_path: &str) -> Result<(), String> {
    let mut pairs = load_pairs();
    if let Some(p) = pairs.get_mut(local_path) {
        p.initial_resync_done = true;
        p.last_sync_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        save_pairs(&pairs)?;
    }
    Ok(())
}

/// Build the args for `rclone bisync` for a given pair.
///
/// First run: pass `--resync` to build the initial listings.
/// Later runs: pass `--recover` so an interrupted prior run can be salvaged.
/// `--recover` and `--resync` are mutually exclusive — passing both confuses
/// some rclone versions.
pub fn bisync_args(pair: &SyncPair) -> Vec<String> {
    let mut args = vec![
        "bisync".to_string(),
        pair.sync_local_path.clone(),
        pair.remote.clone(),
        "--verbose".to_string(),
        "--stats".to_string(),
        "5s".to_string(),
        "--stats-one-line".to_string(),
        "--conflict-resolve".to_string(),
        "newer".to_string(),
        "--conflict-suffix".to_string(),
        "conflict-local,conflict-remote".to_string(),
        "--resilient".to_string(),
        // (Not --one-file-system: it would leave out btrfs subvolumes and
        // bind mounts too, and bisync would delete their files on the
        // remote. Mounts inside the folder are refused when it's set up.)
    ];
    if pair.initial_resync_done {
        args.push("--recover".to_string());
    } else {
        args.push("--resync".to_string());
    }
    args
}

/// Stamp the pair as synced (best-effort). Public so the dialog can call it
/// from inside the success-message closure once rclone exits cleanly.
pub fn record_successful_sync(local_path: &str) {
    let _ = mark_synced(local_path);
}

/// Count conflict files in captured rclone output. rclone bisync logs each
/// conflict with an action verb (`Copied`, `Renaming`/`renaming`) so we
/// require both the suffix AND the verb to avoid false positives from
/// informational lines like "Skipping foo.conflict-local: already exists".
///
/// We anchor on `.conflict-local` only (not `.conflict-remote`) since each
/// conflict creates one of each — counting both would double-count.
pub fn count_conflicts(stdout: &str, stderr: &str) -> usize {
    stderr
        .lines()
        .chain(stdout.lines())
        .filter(|line| {
            line.contains(".conflict-local")
                && (line.contains(": Copied")
                    || line.contains("Renaming")
                    || line.contains("renaming"))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::remote_refusal;
    use std::path::Path;

    #[test]
    fn a_mount_folders_sync_is_with_its_own_remote_only() {
        let cache = Path::new("/home/u/.cache/wayfinder/offline");
        let copy = Path::new("/home/u/.cache/wayfinder/offline/home/u/proton/Work");
        // Mounted: its own remote only, even with no pairing before (one
        // removed and set up again).
        assert!(remote_refusal(copy, "Proton:Work", Some("Proton:Work"), None, cache).is_none());
        assert!(
            remote_refusal(copy, "Proton:Personal", Some("Proton:Work"), None, cache).is_some()
        );
        // Paired with another before (an older version allowed it): its
        // copy holds that one's files.
        assert!(remote_refusal(
            copy,
            "Proton:Work",
            Some("Proton:Work"),
            Some("Proton:Personal"),
            cache
        )
        .is_some());
        // Not mounted: the remote it was paired with.
        assert!(
            remote_refusal(copy, "Proton:Personal", None, Some("Proton:Work"), cache).is_some()
        );
        assert!(remote_refusal(copy, "Proton:Work", None, Some("Proton:Work"), cache).is_none());
        // A folder of the user's own may sync with any remote.
        let own = Path::new("/home/u/Work");
        assert!(remote_refusal(
            own,
            "Proton:Personal",
            Some("Proton:Work"),
            Some("X:"),
            cache
        )
        .is_none());
    }
}
