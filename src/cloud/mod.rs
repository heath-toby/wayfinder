//! Wayfinder Sync: offline copies kept in step with their cloud folders,
//! both ways, by the `wayfinder-sync` service.
//!
//! A *drive* is an rclone remote's top level (`Proton:`) and where it's
//! mounted (`~/protonDrive`), set up once in Wayfinder's guide and kept in
//! `~/.config/wayfinder/drives`. Every folder or file kept offline on a
//! drive is a *sync item*: its offline copy (in `offline::cache_root`) and
//! its place on the remote, synced with `rclone bisync` — which also
//! carries over moves and deletions, and keeps both copies of a file
//! changed on both sides. This module holds the logic, free of GTK and of
//! the service's event loop, so it can be tested.

use std::path::{Path, PathBuf};

/// A cloud drive the service looks after.
#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    /// The rclone remote's top level, e.g. `Proton:`.
    pub remote: String,
    /// Where it's mounted, e.g. `/home/Toby/protonDrive`.
    pub mountpoint: String,
    /// The service runs (and restarts) the mount itself.
    pub manage_mount: bool,
    /// The managed mount's remote-control port.
    pub rc_port: u16,
}

impl Drive {
    /// The remote's name for messages: `Proton` for `Proton:`.
    pub fn name(&self) -> &str {
        self.remote.split(':').next().unwrap_or(&self.remote)
    }
}

/// `~/.config/wayfinder/drives`.
pub fn drives_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wayfinder")
        .join("drives")
}

/// The drives in the drives file's text: one per line, tab-separated
/// `remote  mountpoint  managed(0/1)  rc-port`; `#` lines are comments.
/// Lines that don't read are skipped.
pub fn parse_drives(text: &str) -> Vec<Drive> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let (remote, mountpoint) = (f.first()?.trim(), f.get(1)?.trim());
            if !remote.contains(':') || !mountpoint.starts_with('/') {
                return None;
            }
            Some(Drive {
                remote: remote.to_string(),
                mountpoint: mountpoint.trim_end_matches('/').to_string(),
                manage_mount: f.get(2).is_some_and(|v| v.trim() == "1"),
                rc_port: f.get(3).and_then(|v| v.trim().parse().ok()).unwrap_or(5572),
            })
        })
        .collect()
}

pub fn format_drives(drives: &[Drive]) -> String {
    let mut out = String::from(
        "# Wayfinder Sync drives — remote<TAB>mountpoint<TAB>managed mount (0/1)<TAB>remote-control port\n",
    );
    for d in drives {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            d.remote,
            d.mountpoint,
            u8::from(d.manage_mount),
            d.rc_port
        ));
    }
    out
}

pub fn load_drives() -> Vec<Drive> {
    std::fs::read_to_string(drives_path())
        .map(|t| parse_drives(&t))
        .unwrap_or_default()
}

/// Write the drives file whole (to a temporary file, then renamed), so a
/// crash can't leave it half written.
pub fn save_drives(drives: &[Drive]) -> Result<(), String> {
    let path = drives_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, format_drives(drives)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// The drive `path` is on, if any (the innermost, should one be mounted
/// inside another).
pub fn drive_for<'a>(path: &str, drives: &'a [Drive]) -> Option<&'a Drive> {
    drives
        .iter()
        .filter(|d| path == d.mountpoint || path.starts_with(&format!("{}/", d.mountpoint)))
        .max_by_key(|d| d.mountpoint.len())
}

/// Where `path`, on `drive`, is on the remote: `Proton:Documents 2/German`.
pub fn remote_spec(drive: &Drive, path: &str) -> String {
    let rel = path
        .strip_prefix(&drive.mountpoint)
        .unwrap_or("")
        .trim_start_matches('/');
    if rel.is_empty() {
        drive.remote.clone()
    } else if drive.remote.ends_with(':') || drive.remote.ends_with('/') {
        format!("{}{rel}", drive.remote)
    } else {
        format!("{}/{rel}", drive.remote)
    }
}

/// Something kept offline, and synced.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncItem {
    /// Where it is on the drive: `/home/Toby/protonDrive/Documents 2/German`.
    pub original: String,
    /// Its offline copy.
    pub local: PathBuf,
    /// Its place on the remote: `Proton:Documents 2/German`.
    pub remote: String,
    pub is_dir: bool,
}

impl SyncItem {
    /// Its name for messages: `German`.
    pub fn name(&self) -> String {
        Path::new(&self.original)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.original.clone())
    }
}

/// The sync items: everything kept offline (`pins`: original path, is a
/// folder) on one of `drives`, plus two-way sync pairings made before
/// (`pairs`: user-facing path, remote, local folder) whose local folder is
/// an offline copy — one item per place, the pairing's remote kept if
/// both name it. Offline copies live under `cache_root`, mirroring the
/// original's full path.
pub fn sync_items(
    drives: &[Drive],
    pins: &[(String, bool)],
    pairs: &[(String, String, String)],
    cache_root: &Path,
) -> Vec<SyncItem> {
    let copy_of = |original: &str| cache_root.join(original.trim_start_matches('/'));
    let mut items: Vec<SyncItem> = Vec::new();
    for (local_key, remote, sync_local) in pairs {
        if drive_for(local_key, drives).is_none() || Path::new(sync_local) != copy_of(local_key) {
            continue;
        }
        items.push(SyncItem {
            original: local_key.clone(),
            local: copy_of(local_key),
            remote: remote.trim_end_matches('/').to_string(),
            is_dir: true,
        });
    }
    for (original, is_dir) in pins {
        let Some(drive) = drive_for(original, drives) else {
            continue;
        };
        if items.iter().any(|i| i.original == *original) {
            continue;
        }
        items.push(SyncItem {
            original: original.clone(),
            local: copy_of(original),
            remote: remote_spec(drive, original),
            is_dir: *is_dir,
        });
    }
    // (One inside another would be synced twice, both ways: the outer
    // covers it.)
    let outer: Vec<String> = items.iter().map(|i| i.original.clone()).collect();
    items.retain(|i| {
        !outer
            .iter()
            .any(|o| *o != i.original && i.original.starts_with(&format!("{o}/")))
    });
    items.sort_by(|a, b| a.original.cmp(&b.original));
    items
}

/// Names never synced: Wayfinder's own part-downloads, and the lock files
/// LibreOffice keeps beside an open document (synced, they'd make it look
/// open elsewhere).
pub fn is_ignored(name: &str) -> bool {
    name.ends_with(".wayfinder-part")
        || (name.starts_with(".~lock.") && name.ends_with('#'))
        || name.ends_with(".partial")
}

/// The item a changed path in the offline copies belongs to.
pub fn item_for_change<'a>(path: &Path, items: &'a [SyncItem]) -> Option<&'a SyncItem> {
    if path
        .file_name()
        .is_some_and(|n| is_ignored(&n.to_string_lossy()))
    {
        return None;
    }
    items.iter().find(|i| path == i.local || path.starts_with(&i.local))
}

/// A short, stable name for the item (from its original path), for the
/// rclone aliases below.
pub fn item_id(item: &SyncItem) -> String {
    // FNV-1a: stable across runs and builds (std's hasher isn't).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in item.original.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The short path bisync is given for an item's offline copy: a link,
/// `<state>/wayfinder/sync/<id>`, to it. bisync names its state files after
/// both of its paths joined together, so a deep folder's offline copy —
/// whose path repeats the original's in full under the cache — passes the
/// 255-byte limit on a file name and fails every run. (rclone takes the
/// folder by the path it's given, so the names come from the link.)
pub fn short_local(item: &SyncItem, state_dir: &Path) -> PathBuf {
    state_dir.join("wayfinder").join("sync").join(item_id(item))
}

/// The `rclone` arguments to sync a folder item, its offline copy given
/// as `local` (its `short_local` link). `first` builds the baseline
/// (`--resync`: copies what's missing either way, deletes nothing) —
/// needed once, before any other run.
pub fn bisync_args(item: &SyncItem, local: &Path, first: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "bisync",
        &local.to_string_lossy(),
        &item.remote,
        "--verbose",
        "--color",
        "NEVER",
        // Changed on both sides: the newer wins, the other is kept under a
        // new name (…conflict-local1), and the user is told.
        "--conflict-resolve",
        "newer",
        "--conflict-suffix",
        "conflict-local,conflict-remote",
        "--resilient",
        // A run that was stopped (by sleep, a crash) leaves its lock
        // behind, which would refuse every run after it: let it expire.
        "--max-lock",
        "5m",
        "--exclude",
        "*.wayfinder-part",
        "--exclude",
        ".~lock.*#",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(if first { "--resync" } else { "--recover" }.to_string());
    args
}

/// Which way a single kept-offline file is copied: bisync works on folders
/// only, so a file is synced by comparing its two copies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileSync {
    /// The remote's copy is newer (or the only one): fetch it.
    Fetch,
    /// The offline copy is newer (or the only one): send it.
    Send,
    Nothing,
}

/// For a file's offline and remote copies (modification time in seconds,
/// size), which way to copy. Times within two seconds count as the same
/// (remotes round them); then a size difference means the remote's copy
/// wins, being the one shared with other devices.
pub fn file_sync(local: Option<(i64, u64)>, remote: Option<(i64, u64)>) -> FileSync {
    match (local, remote) {
        (None, None) => FileSync::Nothing,
        (None, Some(_)) => FileSync::Fetch,
        (Some(_), None) => FileSync::Send,
        (Some((lt, ls)), Some((rt, rs))) => {
            if (lt - rt).abs() <= 2 {
                if ls == rs {
                    FileSync::Nothing
                } else {
                    FileSync::Fetch
                }
            } else if lt > rt {
                FileSync::Send
            } else {
                FileSync::Fetch
            }
        }
    }
}

/// What a side of a sync found changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Changes {
    pub new: usize,
    pub modified: usize,
    pub deleted: usize,
}

impl Changes {
    pub fn total(&self) -> usize {
        self.new + self.modified + self.deleted
    }
}

/// What a bisync run did, read from its output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    /// Never synced before (no listings): run again with `--resync`.
    pub needs_first_run: bool,
    /// Changed in the offline copy (Path1), so sent to the remote.
    pub sent: Changes,
    /// Changed on the remote (Path2), so fetched.
    pub fetched: Changes,
    /// Files changed on both sides: both copies kept.
    pub conflicts: usize,
    /// The run's own reason for failing.
    pub error: Option<String>,
}

/// Read a `bisync --verbose --color NEVER` run's output.
///
/// Changes are counted from what the run did, not what it noticed: after
/// "Do queued copies to Path1" come the copies and deletions made in the
/// offline copy (fetched), after "… to Path2" those made on the remote
/// (sent). (What it notices can be nothing to do: the run after a
/// conflict sees the winner "new" on both sides, finds them equal, and
/// skips it.) Conflict copies are counted as conflicts, not changes.
pub fn parse_bisync(output: &str) -> Outcome {
    let mut out = Outcome::default();
    // Which side the copies being made land on: Path1 (true) or Path2.
    let mut into_local: Option<bool> = None;
    for line in output.lines() {
        if line.contains("Bisync successful") {
            out.ok = true;
        } else if line.contains("cannot find prior Path1 or Path2 listings") {
            out.needs_first_run = true;
        } else if line.contains("Do queued copies to") {
            into_local = Some(line.trim_end().ends_with("Path1"));
        } else if line.contains("Renaming Path1 copy") || line.contains("Renaming Path2 copy") {
            out.conflicts += 1;
        } else if line.contains("Updating listings") {
            into_local = None;
        } else if out.error.is_none() && line.contains("ERROR : ") && line.contains("Bisync") {
            out.error = line.split_once("ERROR : ").map(|(_, e)| e.trim().to_string());
        } else if let Some((_, why)) = line.split_once("Failed to bisync: ") {
            // (The run's last word on it; an earlier, fuller reason wins.)
            if out.error.is_none() {
                out.error = Some(why.trim().to_string());
            }
        } else if let Some(local) = into_local {
            let Some((_, rest)) = line.split_once(" : ") else {
                continue;
            };
            let Some((name, what)) = rest.rsplit_once(": ") else {
                continue;
            };
            if name.contains(".conflict-local") || name.contains(".conflict-remote") {
                continue;
            }
            let side = if local { &mut out.fetched } else { &mut out.sent };
            match what.trim() {
                "Copied (new)" => side.new += 1,
                "Copied (replaced existing)" => side.modified += 1,
                "Deleted" => side.deleted += 1,
                _ => {}
            }
        }
    }
    out
}

/// `n` and a noun, made plural when it should be: "1 file", "2 files".
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// What to tell the user about a sync of `item` with `drive_name`'s
/// remote: `None` if nothing changed (a check that found nothing isn't
/// worth hearing about).
pub fn describe(item_name: &str, drive_name: &str, outcome: &Outcome) -> Option<String> {
    if !outcome.ok {
        let why = outcome
            .error
            .as_deref()
            .unwrap_or("the sync didn't finish");
        return Some(format!(
            "Couldn't sync {item_name} with {drive_name}: {why}. Wayfinder will try again."
        ));
    }
    let mut parts = Vec::new();
    if outcome.fetched.total() > 0 {
        parts.push(format!("{} from {drive_name}", count(outcome.fetched.total(), "change")));
    }
    if outcome.sent.total() > 0 {
        parts.push(format!("{} sent to {drive_name}", count(outcome.sent.total(), "change")));
    }
    if outcome.conflicts > 0 {
        parts.push(format!(
            "{} changed in both places: both copies kept",
            count(outcome.conflicts, "file")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("Sync updated: {item_name}, {}.", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proton() -> Drive {
        Drive {
            remote: "Proton:".into(),
            mountpoint: "/home/u/protonDrive".into(),
            manage_mount: true,
            rc_port: 5572,
        }
    }

    #[test]
    fn drives_file_round_trip() {
        let drives = vec![
            proton(),
            Drive {
                remote: "gdrive:Work".into(),
                mountpoint: "/mnt/g".into(),
                manage_mount: false,
                rc_port: 5573,
            },
        ];
        assert_eq!(parse_drives(&format_drives(&drives)), drives);
        // A trailing slash on the mountpoint, and junk lines.
        let text = "# x\nProton:\t/home/u/protonDrive/\t1\t5572\nnonsense\n";
        assert_eq!(parse_drives(text), vec![proton()]);
    }

    #[test]
    fn remote_spec_of_paths() {
        let d = proton();
        assert_eq!(remote_spec(&d, "/home/u/protonDrive"), "Proton:");
        assert_eq!(
            remote_spec(&d, "/home/u/protonDrive/Documents 2/German"),
            "Proton:Documents 2/German"
        );
        let sub = Drive {
            remote: "gdrive:Work".into(),
            ..proton()
        };
        assert_eq!(remote_spec(&sub, "/home/u/protonDrive/a"), "gdrive:Work/a");
    }

    #[test]
    fn items_from_pins_and_pairs() {
        let cache = Path::new("/home/u/.cache/wayfinder/offline");
        let pins = vec![
            ("/home/u/protonDrive/Documents 2/German".to_string(), true),
            ("/home/u/protonDrive/Audio/a.m4a".to_string(), false),
            // Not on a drive the service looks after.
            ("/media/usb/x".to_string(), true),
            // Inside another item: covered by it.
            ("/home/u/protonDrive/Documents 2/German/Literature".to_string(), true),
        ];
        let pairs = vec![(
            "/home/u/protonDrive/Documents 2/Russian".to_string(),
            "Proton:Documents 2/Russian/".to_string(),
            "/home/u/.cache/wayfinder/offline/home/u/protonDrive/Documents 2/Russian".to_string(),
        )];
        let items = sync_items(&[proton()], &pins, &pairs, cache);
        let got: Vec<(&str, &str, bool)> = items
            .iter()
            .map(|i| (i.original.as_str(), i.remote.as_str(), i.is_dir))
            .collect();
        assert_eq!(
            got,
            vec![
                ("/home/u/protonDrive/Audio/a.m4a", "Proton:Audio/a.m4a", false),
                ("/home/u/protonDrive/Documents 2/German", "Proton:Documents 2/German", true),
                ("/home/u/protonDrive/Documents 2/Russian", "Proton:Documents 2/Russian", true),
            ]
        );
        assert_eq!(
            items[1].local,
            cache.join("home/u/protonDrive/Documents 2/German")
        );
    }

    #[test]
    fn changes_map_to_their_item() {
        let cache = Path::new("/c");
        let pins = vec![("/home/u/protonDrive/German".to_string(), true)];
        let items = sync_items(&[proton()], &pins, &[], cache);
        let doc = Path::new("/c/home/u/protonDrive/German/essay.odt");
        assert_eq!(item_for_change(doc, &items).map(|i| i.name()), Some("German".into()));
        for ignored in [
            "/c/home/u/protonDrive/German/.~lock.essay.odt#",
            "/c/home/u/protonDrive/German/big.pdf.wayfinder-part",
            "/c/home/u/protonDrive/Russian/x",
        ] {
            assert_eq!(item_for_change(Path::new(ignored), &items), None, "{ignored}");
        }
    }

    /// A real run: a.txt changed on both sides (remote newer), l.txt
    /// changed locally, b.txt deleted and new.txt added on the remote.
    const RUN: &str = "\
INFO  : - Path1    File changed: size (larger), time (newer) - a.txt
INFO  : - Path1    File changed: size (larger), time (newer) - l.txt
INFO  : Path1:    2 changes:    0 new,    2 modified,    0 deleted
INFO  : - Path2    File changed: size (larger), time (newer) - a.txt
INFO  : - Path2    File was deleted          - b.txt
INFO  : - Path2    File is new               - new.txt
INFO  : Path2:    3 changes:    1 new,    1 modified,    1 deleted
NOTICE: - Path1    Renaming Path1 copy                - l/a.txt.conflict-local1
INFO  : a.txt: Moved (server-side) to: a.txt.conflict-local1
NOTICE: - Path1    Queue copy to Path2       - r/a.txt.conflict-local1
NOTICE: - Path2    Not renaming Path2 copy, as it was determined the winner - r/a.txt
NOTICE: - Path2    Queue copy to Path1       - l/a.txt
INFO  : - Path2    Do queued copies to                - Path1
INFO  : a.txt: Copied (new)
INFO  : new.txt: Copied (new)
INFO  : b.txt: Deleted
INFO  : - Path1    Do queued copies to                - Path2
INFO  : a.txt.conflict-local1: Copied (new)
INFO  : l.txt: Copied (replaced existing)
INFO  : Updating listings
INFO  : Bisync successful
";

    #[test]
    fn reads_a_run() {
        let o = parse_bisync(RUN);
        assert!(o.ok);
        assert_eq!(o.sent, Changes { new: 0, modified: 1, deleted: 0 });
        assert_eq!(o.fetched, Changes { new: 2, modified: 0, deleted: 1 });
        assert_eq!(o.conflicts, 1);
        assert_eq!(
            describe("German", "Proton", &o).as_deref(),
            Some("Sync updated: German, 3 changes from Proton, 1 change sent to Proton, 1 file changed in both places: both copies kept.")
        );
    }

    /// The run after a conflict records the copies it made: not news.
    #[test]
    fn conflict_copies_recorded_later_are_not_news() {
        let text = "\
INFO  : - Path1    File is new               - a.txt
INFO  : - Path2    File is new               - a.txt
INFO  : Files are equal! Skipping: a.txt
INFO  : Updating listings
INFO  : Bisync successful
";
        let o = parse_bisync(text);
        assert!(o.ok);
        assert_eq!(describe("Docs", "Proton", &o), None);
    }

    #[test]
    fn nothing_changed_says_nothing() {
        let o = parse_bisync("INFO  : No changes found\nINFO  : Bisync successful\n");
        assert!(o.ok);
        assert_eq!(describe("German", "Proton", &o), None);
    }

    #[test]
    fn first_run_and_failures() {
        let text = "ERROR : Bisync critical error: cannot find prior Path1 or Path2 listings, likely due to critical error on prior run\nNOTICE: Failed to bisync: bisync aborted\n";
        let o = parse_bisync(text);
        assert!(!o.ok && o.needs_first_run);
        let o = parse_bisync("NOTICE: Failed to bisync: prior lock file found: x.lck\n");
        assert_eq!(o.error.as_deref(), Some("prior lock file found: x.lck"));
        let o = parse_bisync("ERROR : Bisync critical error: couldn't list Proton: no internet\n");
        assert_eq!(
            describe("German", "Proton", &o).as_deref(),
            Some("Couldn't sync German with Proton: Bisync critical error: couldn't list Proton: no internet. Wayfinder will try again.")
        );
    }

    #[test]
    fn single_file_direction() {
        assert_eq!(file_sync(Some((100, 5)), Some((100, 5))), FileSync::Nothing);
        assert_eq!(file_sync(Some((101, 5)), Some((100, 5))), FileSync::Nothing);
        assert_eq!(file_sync(Some((200, 5)), Some((100, 5))), FileSync::Send);
        assert_eq!(file_sync(Some((100, 5)), Some((200, 9))), FileSync::Fetch);
        assert_eq!(file_sync(Some((100, 5)), Some((100, 9))), FileSync::Fetch);
        assert_eq!(file_sync(None, Some((1, 1))), FileSync::Fetch);
        assert_eq!(file_sync(Some((1, 1)), None), FileSync::Send);
    }

    #[test]
    fn folder_bisync_args() {
        let item = SyncItem {
            original: "/home/u/protonDrive/German".into(),
            local: "/c/German".into(),
            remote: "Proton:German".into(),
            is_dir: true,
        };
        let link = short_local(&item, Path::new("/home/u/.local/state"));
        let id = item_id(&item);
        assert_eq!(id.len(), 16);
        assert_eq!(link, Path::new("/home/u/.local/state/wayfinder/sync").join(&id));
        let args = bisync_args(&item, &link, true);
        assert_eq!(&args[..3], ["bisync", link.to_str().unwrap(), "Proton:German"]);
        assert!(args.contains(&"--max-lock".to_string()));
        assert!(args.contains(&"--resync".to_string()));
        assert!(bisync_args(&item, &link, false).contains(&"--recover".to_string()));
    }
}
