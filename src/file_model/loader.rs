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
    let is_fuse = crate::rclone_ops::is_fuse_path(&real.to_string_lossy());

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
        is_fuse: false,
        entries,
    })
}

/// Look up recently used files, skipping ones that no longer exist.
pub(super) fn list_recent(paths: Vec<String>, limit: usize) -> ListResult {
    let mut entries = Vec::new();
    for path in paths {
        let is_fuse = crate::rclone_ops::is_fuse_path(&path);
        let Ok(info) = gio::File::for_path(&path).query_info(
            attrs_for(is_fuse),
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        ) else {
            continue;
        };
        let parent = std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        entries.push((parent, FileInfoData::from_file_info(&info)));
        if entries.len() >= limit {
            break;
        }
    }
    Ok(Listing {
        path: "recent:///".to_string(),
        is_fuse: false,
        entries,
    })
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

thread_local! {
    static CACHE: RefCell<HashMap<String, (Instant, Vec<FileInfoData>)>> =
        RefCell::new(HashMap::new());
    static PREFETCHING: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn cached(path: &str) -> Option<Vec<FileInfoData>> {
    CACHE.with(|c| c.borrow().get(path).map(|(_, entries)| entries.clone()))
}

pub(super) fn store(path: &str, entries: Vec<FileInfoData>) {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.insert(path.to_string(), (Instant::now(), entries));
        if cache.len() > CACHE_LIMIT {
            let oldest = cache
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            }
        }
    });
}

/// Drop a location's cached listing (it can't be listed any more).
pub(super) fn forget(path: &str) {
    CACHE.with(|c| {
        c.borrow_mut().remove(path);
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
            .get(path)
            .is_some_and(|(at, _)| at.elapsed().as_secs() < 30)
    });
    if fresh || PREFETCHING.with(Cell::get) {
        return;
    }
    PREFETCHING.with(|p| p.set(true));
    let path = path.to_string();
    let rx = {
        let path = path.clone();
        spawn(move || list_dir(&path, false))
    };
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
            Ok(Ok(listing)) if listing.is_fuse => {
                store(&path, listing.entries.into_iter().map(|(_, e)| e).collect());
            }
            _ => {}
        }
        PREFETCHING.with(|p| p.set(false));
        gtk::glib::ControlFlow::Break
    });
}
