//! Background directory listing.
//!
//! Listing a folder on a FUSE mount can take seconds (or forever, if the
//! daemon is stuck), so it never happens on the GTK main thread. Each job
//! runs on its own thread and produces plain data; the model turns it into
//! `FileObject`s on the main thread. A job that is no longer wanted (the
//! user went elsewhere) simply has its result dropped — the syscall itself
//! can't be cancelled.
//!
//! FUSE listings are also kept in a small cache, so going back into a
//! folder shows it at once while a fresh listing is fetched.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use gtk::gio;
use gtk::prelude::*;

use crate::file_object::FileInfoData;

/// Attributes for local folders. `standard::*` includes a content type
/// sniffed from the first bytes of a file when its name doesn't settle it.
const LOCAL_ATTRS: &str = "standard::*,time::modified";

/// Attributes for FUSE folders. Sniffing reads every ambiguous file, which
/// on rclone is a download per file, so the type is guessed from the name.
const FUSE_ATTRS: &str = "standard::name,standard::display-name,standard::type,\
standard::size,standard::is-hidden,standard::is-symlink,standard::fast-content-type,\
time::modified";

pub(super) fn attrs_for(is_fuse: bool) -> &'static str {
    if is_fuse {
        FUSE_ATTRS
    } else {
        LOCAL_ATTRS
    }
}

/// The result of a listing job.
pub struct Listing {
    /// The location actually listed: the requested one, or its nearest
    /// existing ancestor when the job was asked to resolve.
    pub path: String,
    /// `path` with symlinks resolved: where it really is.
    pub real_path: String,
    pub is_fuse: bool,
    /// Each entry with the folder it lives in (they differ for Recent).
    pub entries: Vec<(String, FileInfoData)>,
}

pub type ListResult = Result<Listing, String>;

/// List a directory. With `resolve`, a path that isn't a directory (gone,
/// or a file) is walked up to the nearest one that is.
pub(super) fn list_dir(path: &str, resolve: bool) -> ListResult {
    let mut dir = PathBuf::from(path);
    if resolve {
        while !dir.is_dir() {
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => {
                    dir = PathBuf::from("/");
                    break;
                }
            }
        }
    }
    let path = dir.to_string_lossy().to_string();
    // Decide on the real location, so a symlink into a mount counts as FUSE.
    let real = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let real_path = real.to_string_lossy().to_string();
    let is_fuse = crate::rclone_ops::is_fuse_path(&real_path);

    let enumerator = gio::File::for_path(&dir)
        .enumerate_children(
            attrs_for(is_fuse),
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    while let Some(info) = enumerator
        .next_file(gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?
    {
        entries.push((path.clone(), FileInfoData::from_file_info(&info)));
    }
    Ok(Listing {
        path,
        real_path,
        is_fuse,
        entries,
    })
}

/// List a GIO location such as `trash:///`.
pub(super) fn list_uri(uri: &str) -> ListResult {
    let enumerator = gio::File::for_uri(uri)
        .enumerate_children(
            LOCAL_ATTRS,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    while let Some(info) = enumerator
        .next_file(gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?
    {
        entries.push((uri.to_string(), FileInfoData::from_file_info(&info)));
    }
    Ok(Listing {
        path: uri.to_string(),
        real_path: uri.to_string(),
        is_fuse: false,
        entries,
    })
}

/// Look up recently used files, skipping ones that no longer exist.
///
/// Files on FUSE or network mounts (by where they really are, links
/// resolved) are asked about in parallel and waited for only briefly: a
/// mount that has stopped answering mustn't hold up the list. One that
/// hasn't answered in time — or can't be reached — is listed by name (its
/// type from that; size and date unknown); only one the mount says isn't
/// there is left out. A mount with a question still unanswered from
/// before isn't asked again until it answers, so stuck threads can't pile
/// up beyond one listing's worth.
pub(super) fn list_recent(paths: Vec<String>, limit: usize) -> ListResult {
    // The mount table, read once for the whole list. Innermost first, so a
    // mount inside another is found as itself.
    let mut mounts: Vec<std::path::PathBuf> = super::remote_mountpoints().into_iter().collect();
    mounts.sort_by_key(|m| std::cmp::Reverse(m.as_os_str().len()));
    let busy = busy_mounts();

    let mut slots: Vec<Option<(String, FileInfoData)>> = Vec::new();
    let (tx, rx) = mpsc::channel::<(usize, Option<FileInfoData>)>();
    let mut asked = 0;
    // Looked at past the limit: some may turn out to be gone.
    for path in paths.into_iter().take(limit * 2) {
        let p = std::path::Path::new(&path);
        let Some(parent) = p.parent().map(|d| d.to_string_lossy().to_string()) else {
            continue;
        };
        let real = crate::rclone_ops::resolve_links_outside_mounts(p);
        if let Some(mount) = mounts.iter().find(|m| real.starts_with(m)).cloned() {
            // Listed by name now; replaced by what the mount says, if it
            // says in time.
            let index = slots.len();
            slots.push(Some((parent, name_only(p))));
            if !busy.contains(&mount) {
                asked += 1;
                let tx = tx.clone();
                let question = MountQuestion::start(&mount);
                std::thread::spawn(move || {
                    let found = match gio::File::for_path(&path).query_info(
                        FUSE_ATTRS,
                        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                        gio::Cancellable::NONE,
                    ) {
                        Ok(info) => Some(Some(FileInfoData::from_file_info(&info))),
                        // Gone: left out.
                        Err(e) if e.matches(gio::IOErrorEnum::NotFound) => Some(None),
                        // Can't tell (mount dropped): keep it by name.
                        Err(_) => None,
                    };
                    drop(question);
                    if let Some(found) = found {
                        let _ = tx.send((index, found));
                    }
                });
            }
            continue;
        }
        let Ok(info) = gio::File::for_path(&path).query_info(
            LOCAL_ATTRS,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        ) else {
            continue;
        };
        slots.push(Some((parent, FileInfoData::from_file_info(&info))));
    }
    drop(tx);

    // One short wait for all of them together.
    let deadline = Instant::now() + std::time::Duration::from_millis(300);
    while asked > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok((index, found)) = rx.recv_timeout(left) else {
            break;
        };
        asked -= 1;
        match found {
            Some(data) => {
                if let Some(slot) = slots[index].as_mut() {
                    slot.1 = data;
                }
            }
            None => slots[index] = None,
        }
    }

    let entries: Vec<(String, FileInfoData)> = slots.into_iter().flatten().take(limit).collect();
    Ok(Listing {
        path: "recent:///".to_string(),
        real_path: "recent:///".to_string(),
        is_fuse: false,
        entries,
    })
}

/// An entry for `path` from its name alone.
fn name_only(path: &std::path::Path) -> FileInfoData {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    FileInfoData {
        display_name: name.clone(),
        is_hidden: name.starts_with('.'),
        name,
        is_directory: false,
        size: 0,
        modified: 0,
        modified_display: String::new(),
        content_type: content_type.to_string(),
    }
}

/// Mounts with Recent Files questions unanswered, and how many.
static BUSY_MOUNTS: std::sync::Mutex<Vec<(std::path::PathBuf, u32)>> =
    std::sync::Mutex::new(Vec::new());

fn busy_mounts() -> Vec<std::path::PathBuf> {
    BUSY_MOUNTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(mount, _)| mount.clone())
        .collect()
}

/// A question being asked of a mount, while alive.
struct MountQuestion(std::path::PathBuf);

impl MountQuestion {
    fn start(mount: &std::path::Path) -> Self {
        let mut busy = BUSY_MOUNTS.lock().unwrap_or_else(|e| e.into_inner());
        match busy.iter_mut().find(|(m, _)| m == mount) {
            Some((_, count)) => *count += 1,
            None => busy.push((mount.to_path_buf(), 1)),
        }
        Self(mount.to_path_buf())
    }
}

impl Drop for MountQuestion {
    fn drop(&mut self) {
        let mut busy = BUSY_MOUNTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = busy.iter().position(|(m, _)| *m == self.0) {
            busy[i].1 -= 1;
            if busy[i].1 == 0 {
                busy.swap_remove(i);
            }
        }
    }
}

/// Run `job` on its own thread.
pub(super) fn spawn(
    job: impl FnOnce() -> ListResult + Send + 'static,
) -> mpsc::Receiver<ListResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    rx
}

// -----------------------------------------------------------------
// FUSE listing cache (main thread only)
// -----------------------------------------------------------------

const CACHE_LIMIT: usize = 128;

/// A location's cached listing.
struct Cached {
    at: Instant,
    /// Where the location really is (symlinks resolved).
    real_path: String,
    entries: Vec<FileInfoData>,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Cached>> = RefCell::new(HashMap::new());
    static PREFETCHING: Cell<bool> = const { Cell::new(false) };
    /// Counts the times cached listings were dropped as out of date.
    static CHANGES: Cell<u64> = const { Cell::new(0) };
}

/// A mark to pass to `store`: a listing begun now mustn't be cached if
/// something is forgotten before it's done — it may predate the change.
pub(super) fn changes() -> u64 {
    CHANGES.with(Cell::get)
}

/// The cached listing of `path`, if its mount is still there. Once it has
/// gone (cleanly unmounted, leaving an empty mountpoint), the listing would
/// hide that — and the offline copy that should be shown instead.
/// The cache key for `path`: without a trailing `/` (as Tab completion
/// leaves it), so `/m/Docs/` and `/m/Docs` are one folder.
fn key(path: &str) -> &str {
    match path.trim_end_matches('/') {
        "" => path,
        trimmed => trimmed,
    }
}

pub(super) fn cached(path: &str) -> Option<Vec<FileInfoData>> {
    CACHE.with(|c| {
        let cache = c.borrow();
        let cached = cache.get(key(path))?;
        crate::rclone_ops::is_fuse_path(&cached.real_path).then(|| cached.entries.clone())
    })
}

/// Cache the listing of `listing.path`, begun at the mark `since` (from
/// `changes`) — unless listings have been forgotten since then.
pub(super) fn store(listing: &Listing, since: u64) {
    if changes() != since {
        return;
    }
    let entries = listing.entries.iter().map(|(_, e)| e.clone()).collect();
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.insert(
            key(&listing.path).to_string(),
            Cached {
                at: Instant::now(),
                real_path: listing.real_path.clone(),
                entries,
            },
        );
        if cache.len() > CACHE_LIMIT {
            let oldest = cache
                .iter()
                .min_by_key(|(_, cached)| cached.at)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            }
        }
    });
}

/// Drop a location's cached listing (it can't be listed any more, or has
/// changed) — under whatever path it was reached by, symlinks included.
pub(super) fn forget(path: &str) {
    forget_matching(path, false);
}

/// Drop the cached listings of `path` and every location inside it.
pub(super) fn forget_under(path: &str) {
    forget_matching(path, true);
}

fn forget_matching(path: &str, with_inside: bool) {
    let path = key(path);
    CHANGES.with(|c| c.set(c.get() + 1));
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        // The same folder may be cached under another path (reached
        // through a symlink): match where it really is, too.
        let mut targets = vec![path.to_string()];
        if let Some(cached) = cache.get(path) {
            if key(&cached.real_path) != path {
                targets.push(key(&cached.real_path).to_string());
            }
        }
        let matches = |p: &str| {
            let p = key(p);
            targets
                .iter()
                .any(|t| p == t || (with_inside && p.starts_with(&format!("{t}/"))))
        };
        cache.retain(|location, cached| !matches(location) && !matches(&cached.real_path));
    });
}

/// Fetch a FUSE folder's listing into the cache in the background, so
/// opening it is instant. One prefetch at a time — until its thread really
/// returns, so a mount that has stopped answering doesn't collect a stuck
/// thread per folder — and folders listed in the last 30 seconds are
/// skipped.
pub(super) fn prefetch(path: &str) {
    let fresh = CACHE.with(|c| {
        c.borrow()
            .get(key(path))
            .is_some_and(|cached| cached.at.elapsed().as_secs() < 30)
    });
    if fresh || PREFETCHING.with(Cell::get) {
        return;
    }
    PREFETCHING.with(|p| p.set(true));
    let path = path.to_string();
    let since = changes();
    let rx = {
        let path = path.clone();
        spawn(move || list_dir(&path, false))
    };
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
            Ok(Ok(listing)) if listing.is_fuse => store(&listing, since),
            _ => {}
        }
        PREFETCHING.with(|p| p.set(false));
        gtk::glib::ControlFlow::Break
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(path: &str, real_path: &str) -> Listing {
        Listing {
            path: path.to_string(),
            real_path: real_path.to_string(),
            is_fuse: true,
            entries: vec![(
                path.to_string(),
                FileInfoData {
                    name: "report.txt".to_string(),
                    display_name: "report.txt".to_string(),
                    is_hidden: false,
                    is_directory: false,
                    size: 1,
                    modified: 0,
                    modified_display: String::new(),
                    content_type: String::new(),
                },
            )],
        }
    }

    /// Once a mount has gone, its cached listings aren't shown: the empty
    /// mountpoint (and the offline copy) is what's really there.
    #[test]
    fn cache_is_not_used_once_the_mount_has_gone() {
        let path = "/nonexistent-wayfinder-test/Docs";
        store(&listing(path, path), changes());
        assert!(cached(path).is_none());
    }

    #[test]
    fn removing_a_folder_forgets_everything_inside_it() {
        let cached_at = |p: &str| CACHE.with(|c| c.borrow().contains_key(p));
        for p in [
            "/t-wf/Docs/Old",
            "/t-wf/Docs/Old/Sub",
            "/t-wf/Docs/Older",
            "/t-wf/Docs",
        ] {
            store(&listing(p, p), changes());
        }
        forget_under("/t-wf/Docs/Old");
        assert!(!cached_at("/t-wf/Docs/Old"));
        assert!(!cached_at("/t-wf/Docs/Old/Sub"));
        // A sibling whose name merely starts the same, and the parent, stay.
        assert!(cached_at("/t-wf/Docs/Older"));
        assert!(cached_at("/t-wf/Docs"));
    }

    /// A folder reached through a symlink and directly is one folder.
    #[test]
    fn forgetting_covers_the_same_folder_by_another_path() {
        let cached_at = |p: &str| CACHE.with(|c| c.borrow().contains_key(p));
        let real = "/t-wf-sym/mnt/Docs";
        let link = "/t-wf-sym/home/cloud/Docs";
        store(&listing(real, real), changes());
        store(&listing(link, real), changes());
        forget(link);
        assert!(!cached_at(link));
        assert!(!cached_at(real));
    }

    /// A listing begun before a folder was changed (and forgotten) isn't
    /// cached when it arrives: it may not show the change.
    #[test]
    fn listing_from_before_a_change_is_not_cached() {
        let path = "/t-wf-race/Docs";
        let since = changes();
        forget("/t-wf-race/Elsewhere");
        CACHE.with(|c| c.borrow_mut().remove(path));
        store(&listing(path, path), since);
        assert!(CACHE.with(|c| !c.borrow().contains_key(path)));
        store(&listing(path, path), changes());
        assert!(CACHE.with(|c| c.borrow().contains_key(path)));
    }

    /// `/m/Docs/` (as Tab completion leaves it) and `/m/Docs` are one
    /// folder to the cache.
    #[test]
    fn trailing_slash_is_the_same_folder() {
        let cached_at = |p: &str| CACHE.with(|c| c.borrow().contains_key(p));
        store(
            &listing("/t-wf-slash/Docs/", "/t-wf-slash/Docs/"),
            changes(),
        );
        assert!(cached_at("/t-wf-slash/Docs"));
        forget("/t-wf-slash/Docs");
        assert!(!cached_at("/t-wf-slash/Docs"));
    }

    #[test]
    fn forgotten_listings_are_gone() {
        let path = "/nonexistent-wayfinder-test/Other";
        store(&listing(path, path), changes());
        forget(path);
        assert!(CACHE.with(|c| !c.borrow().contains_key(path)));
    }
}
