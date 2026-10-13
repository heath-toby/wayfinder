mod loader;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{CustomFilter, CustomSorter, FilterChange, FilterListModel, SortListModel};

use crate::file_object::FileObject;
use crate::search::SearchState;

pub use loader::Listing;

/// Check whether a path resides on a FUSE filesystem (e.g. rclone, sshfs),
/// by its mountpoint in `/proc/mounts` (no filesystem access).
fn is_fuse_mount(path: &str) -> bool {
    crate::rclone_ops::is_fuse_path(path)
}

/// How long a load waits for its listing before returning and finishing in
/// the background. Local folders practically always make it, so they still
/// load synchronously; a slow mount never holds the window up for longer.
const SYNC_WAIT: Duration = Duration::from_millis(100);

/// How a finished listing is put into the store.
#[derive(Clone, Copy, PartialEq)]
enum Apply {
    /// The store is empty (a new folder): fill it.
    Replace,
    /// The store shows this folder already: add, remove and update items
    /// in place, so the selection and focus stay where they are.
    Diff,
}

type LoadDone = Box<dyn FnOnce(Result<u32, String>)>;
type Callback = Box<dyn Fn()>;
type ErrorCallback = Box<dyn Fn(String, String)>;

/// Cheap to clone: every clone shares the same store and state.
#[derive(Clone)]
pub struct DirectoryModel {
    pub store: gio::ListStore,
    pub sort_model: SortListModel,
    pub hidden_filter_model: FilterListModel,
    pub filter_model: FilterListModel,
    pub sorter: CustomSorter,
    pub hidden_filter: CustomFilter,
    pub search: SearchState,
    show_hidden: Rc<RefCell<bool>>,
    current_path: Rc<RefCell<String>>,
    monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
    is_fuse: Rc<Cell<bool>>,
    /// Bumped by every navigation; results of older jobs are dropped.
    generation: Rc<Cell<u64>>,
    /// Every listing job gets a sequence number; one that finishes after
    /// a newer one has been applied is stale and dropped.
    next_seq: Rc<Cell<u64>>,
    applied_seq: Rc<Cell<u64>>,
    /// True while a navigation is waiting with nothing to show yet.
    loading: Rc<Cell<bool>>,
    /// Called when a background refresh changed the store on its own.
    on_background_update: Rc<RefCell<Option<Callback>>>,
    /// Called with (location, error) when a background refresh found the
    /// location can no longer be listed; the store has been emptied.
    on_background_error: Rc<RefCell<Option<ErrorCallback>>>,
    /// Background refreshes still waiting for their listing.
    refreshes_in_flight: Rc<Cell<u32>>,
}

impl Default for DirectoryModel {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectoryModel {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<FileObject>();

        let sorter = CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();

            // Directories first
            match (a.is_directory(), b.is_directory()) {
                (true, false) => return gtk::Ordering::Smaller,
                (false, true) => return gtk::Ordering::Larger,
                _ => {}
            }

            // Then alphabetical, case-insensitive (use cached lowercase from search_string)
            let a_lower = a.search_string();
            let b_lower = b.search_string();
            match a_lower.cmp(&b_lower) {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            }
        });

        let sort_model = SortListModel::new(Some(store.clone()), Some(sorter.clone()));

        let show_hidden = Rc::new(RefCell::new(false));
        let show_hidden_ref = show_hidden.clone();

        let hidden_filter = CustomFilter::new(move |obj| {
            let file = obj.downcast_ref::<FileObject>().unwrap();
            if !*show_hidden_ref.borrow() && file.hidden() {
                return false;
            }
            true
        });

        let hidden_filter_model =
            FilterListModel::new(Some(sort_model.clone()), Some(hidden_filter.clone()));

        // Search filter wraps the hidden filter
        let search = SearchState::new();
        let filter_model = FilterListModel::new(
            Some(hidden_filter_model.clone()),
            Some(search.filter.clone()),
        );

        Self {
            store,
            sort_model,
            hidden_filter_model,
            filter_model,
            sorter,
            hidden_filter,
            search,
            show_hidden,
            current_path: Rc::new(RefCell::new(String::new())),
            monitor: Rc::new(RefCell::new(None)),
            is_fuse: Rc::new(Cell::new(false)),
            generation: Rc::new(Cell::new(0)),
            next_seq: Rc::new(Cell::new(0)),
            applied_seq: Rc::new(Cell::new(0)),
            loading: Rc::new(Cell::new(false)),
            on_background_update: Rc::new(RefCell::new(None)),
            on_background_error: Rc::new(RefCell::new(None)),
            refreshes_in_flight: Rc::new(Cell::new(0)),
        }
    }

    /// Load recently used files from GTK's RecentManager.
    pub fn load_recent(&self, on_done: impl FnOnce(Result<u32, String>) + 'static) {
        let gen = self.begin("recent:///", false);
        self.store.remove_all();
        // Only local files; the RecentManager itself is main-thread only.
        let paths = recent_paths();
        self.run(
            gen,
            move || loader::list_recent(paths, 50),
            Apply::Replace,
            Box::new(on_done),
        );
    }

    /// Load a GIO location such as `trash:///`.
    pub fn load_uri(&self, uri: &str, on_done: impl FnOnce(Result<u32, String>) + 'static) {
        let gen = self.begin(uri, false);
        self.store.remove_all();
        let uri = uri.to_string();
        self.run(
            gen,
            move || loader::list_uri(&uri),
            Apply::Replace,
            Box::new(on_done),
        );
    }

    /// Open a directory. `on_done` runs once the listing is in the store —
    /// straight away for local folders and cached FUSE ones, later for a
    /// slow mount, and never if another load supersedes this one. With
    /// `resolve`, a path that no longer exists falls back to its nearest
    /// existing parent; `current_path()` then reports the folder opened.
    pub fn load_directory(
        &self,
        path: &str,
        resolve: bool,
        on_done: impl FnOnce(Result<u32, String>) + 'static,
    ) {
        let gen = self.begin(path, is_fuse_mount(path));
        let job = {
            let path = path.to_string();
            move || loader::list_dir(&path, resolve)
        };

        if let Some(entries) = loader::cached(path) {
            // Show the folder as it was last seen, then check it quietly.
            self.is_fuse.set(true);
            let objects: Vec<glib::Object> = entries
                .iter()
                .map(|e| FileObject::from_data(path, e).upcast())
                .collect();
            self.store.splice(0, self.store.n_items(), &objects);
            on_done(Ok(self.store.n_items()));
            let model = self.clone();
            self.run(
                gen,
                job,
                Apply::Diff,
                Box::new(move |_| {
                    if let Some(cb) = model.on_background_update.borrow().as_ref() {
                        cb();
                    }
                }),
            );
            return;
        }

        self.store.remove_all();
        self.run(gen, job, Apply::Replace, Box::new(on_done));
    }

    /// Like `refresh`, but does nothing while another refresh is still
    /// waiting — so checking for changes each time the window is focused
    /// can't pile up threads on a mount that has stopped answering.
    pub fn refresh_if_idle(&self, on_done: impl FnOnce() + 'static) {
        if self.refreshes_in_flight.get() == 0 {
            self.refresh(on_done);
        }
    }

    /// List the current location again and update the store in place,
    /// keeping the selection. `on_done` runs afterwards — not at all if a
    /// navigation is under way or happens meanwhile.
    pub fn refresh(&self, on_done: impl FnOnce() + 'static) {
        if self.loading.get() {
            // A navigation is still on its way; its listing will be fresh.
            return;
        }
        let location = self.current_path();
        let on_done: LoadDone = Box::new(move |_| on_done());
        let gen = self.generation.get();
        if location == "recent:///" {
            let paths = recent_paths();
            self.run(
                gen,
                move || loader::list_recent(paths, 50),
                Apply::Diff,
                on_done,
            );
        } else if location.contains("://") {
            self.run(
                gen,
                move || loader::list_uri(&location),
                Apply::Diff,
                on_done,
            );
        } else {
            self.run(
                gen,
                move || loader::list_dir(&location, false),
                Apply::Diff,
                on_done,
            );
        }
    }

    /// Whether a navigation is still waiting for its listing.
    pub fn is_loading(&self) -> bool {
        self.loading.get()
    }

    /// Changes with every navigation; lets callers tell whether the load
    /// they started is still the current one.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Called whenever a background check of a cached folder changed it.
    pub fn set_on_background_update(&self, callback: impl Fn() + 'static) {
        *self.on_background_update.borrow_mut() = Some(Box::new(callback));
    }

    /// Called with (location, error) when a background check found the
    /// location can't be listed any more (deleted, mount gone). The store
    /// has been emptied by then.
    pub fn set_on_background_error(&self, callback: impl Fn(String, String) + 'static) {
        *self.on_background_error.borrow_mut() = Some(Box::new(callback));
    }

    /// Warm the cache for a folder inside the current FUSE folder, so
    /// opening it is instant.
    pub fn prefetch(&self, path: &str) {
        if self.is_fuse.get() {
            loader::prefetch(path);
        }
    }

    /// Start a new navigation to `location`.
    fn begin(&self, location: &str, is_fuse: bool) -> u64 {
        log::debug!("load {location} (fuse: {is_fuse})");
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        self.loading.set(false);
        *self.current_path.borrow_mut() = location.to_string();
        self.is_fuse.set(is_fuse);
        *self.monitor.borrow_mut() = None;
        gen
    }

    /// Run a listing job. A navigation (`Apply::Replace`) waits up to
    /// `SYNC_WAIT` before finishing in the background; a refresh never
    /// waits, since the folder is already on screen.
    fn run(
        &self,
        gen: u64,
        job: impl FnOnce() -> loader::ListResult + Send + 'static,
        apply: Apply,
        on_done: LoadDone,
    ) {
        let seq = self.next_seq.get() + 1;
        self.next_seq.set(seq);
        let rx = loader::spawn(job);
        if apply == Apply::Replace {
            match rx.recv_timeout(SYNC_WAIT) {
                Ok(result) => return self.finish(gen, seq, result, apply, on_done),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return self.finish(gen, seq, Err("listing failed".into()), apply, on_done)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => self.loading.set(true),
            }
        } else {
            self.refreshes_in_flight.set(self.refreshes_in_flight.get() + 1);
        }

        let model = self.clone();
        let mut on_done = Some(on_done);
        glib::timeout_add_local(Duration::from_millis(30), move || {
            let result = if model.generation.get() != gen {
                // Superseded. The thread may still be waiting on the mount;
                // its result will be dropped with `rx`.
                None
            } else {
                match rx.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err("listing failed".into())),
                }
            };
            if apply == Apply::Diff {
                model
                    .refreshes_in_flight
                    .set(model.refreshes_in_flight.get().saturating_sub(1));
            }
            if let (Some(result), Some(on_done)) = (result, on_done.take()) {
                model.finish(gen, seq, result, apply, on_done);
            }
            glib::ControlFlow::Break
        });
    }

    fn finish(
        &self,
        gen: u64,
        seq: u64,
        result: loader::ListResult,
        apply: Apply,
        on_done: LoadDone,
    ) {
        if gen != self.generation.get() {
            log::debug!("dropping listing for a location left behind (generation {gen})");
            return;
        }
        if seq < self.applied_seq.get() {
            // A newer listing of this location is already applied, so the
            // store is at least as fresh as this one would make it.
            log::debug!("skipping listing overtaken by a newer one (sequence {seq})");
            on_done(Ok(self.store.n_items()));
            return;
        }
        self.applied_seq.set(seq);
        if apply == Apply::Replace {
            self.loading.set(false);
        }
        let listing = match result {
            Ok(listing) => listing,
            Err(e) => {
                if apply == Apply::Diff {
                    // The location can't be listed any more (deleted, mount
                    // gone): stop showing what it held.
                    let location = self.current_path();
                    self.store.remove_all();
                    loader::forget(&location);
                    if let Some(cb) = self.on_background_error.borrow().as_ref() {
                        cb(location, e.clone());
                    }
                }
                on_done(Err(e));
                return;
            }
        };

        log::debug!("listed {}: {} entries", listing.path, listing.entries.len());
        *self.current_path.borrow_mut() = listing.path.clone();
        self.is_fuse.set(listing.is_fuse);
        if listing.is_fuse {
            loader::store(
                &listing.path,
                listing.entries.iter().map(|(_, e)| e.clone()).collect(),
            );
        }
        match apply {
            Apply::Replace => self.replace_all(&listing),
            Apply::Diff => self.apply_diff(&listing),
        }

        if listing.path.starts_with('/') && !listing.is_fuse {
            // FUSE folders get neither: a monitor's path lookup would touch
            // the mount on this thread (the window refreshes them when it
            // regains focus instead), and sizes would mean walking the
            // remote, so they keep showing a dash.
            self.setup_monitor(&gio::File::for_path(&listing.path));
            self.calculate_folder_sizes();
        }
        on_done(Ok(self.store.n_items()));
    }

    fn replace_all(&self, listing: &Listing) {
        let objects: Vec<glib::Object> = listing
            .entries
            .iter()
            .map(|(parent, data)| {
                let fo = FileObject::from_data(parent, data);
                if data.is_directory && !listing.is_fuse && listing.path.starts_with('/') {
                    fo.set_size_display("Calculating...".to_string());
                }
                fo.upcast()
            })
            .collect();
        self.store.splice(0, self.store.n_items(), &objects);
    }

    fn apply_diff(&self, listing: &Listing) {
        // Keyed by full path: Recent can hold equal names from different folders.
        let mut fresh: std::collections::HashMap<String, FileObject> = listing
            .entries
            .iter()
            .map(|(parent, data)| {
                let fo = FileObject::from_data(parent, data);
                (fo.path(), fo)
            })
            .collect();
        let mut updated = false;
        for i in (0..self.store.n_items()).rev() {
            let Some(fo) = self.store.item(i).and_downcast::<FileObject>() else {
                continue;
            };
            match fresh.remove(&fo.path()) {
                None => self.store.remove(i),
                Some(new) => {
                    if new.is_directory() != fo.is_directory() {
                        self.store.splice(i, 1, &[new]);
                    } else if new.modified() != fo.modified()
                        || (!new.is_directory() && new.size() != fo.size())
                        || new.mime_type() != fo.mime_type()
                    {
                        fo.update_from(&new);
                        updated = true;
                    }
                }
            }
        }
        let added: Vec<glib::Object> = fresh.into_values().map(|fo| fo.upcast()).collect();
        self.store.splice(self.store.n_items(), 0, &added);
        if updated {
            self.resort();
        }
    }

    /// Re-apply the active sort after items changed in place (new items
    /// are placed by the sort model on insertion, changed ones aren't).
    fn resort(&self) {
        if let Some(sorter) = self.sort_model.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
    }

    fn calculate_folder_sizes(&self) {
        // Collect directory paths that need size calculation
        // Never walk into FUSE or network mounts (e.g. ~/protonDrive seen
        // from ~). A thread blocked on an rclone request can't be frozen,
        // so a scan in progress at suspend time makes the suspend fail.
        // Mountpoints are matched by path before any stat, because even
        // stat on a mountpoint is a request to the remote filesystem.
        let skip = remote_mountpoints();
        let base = self.current_path();

        // (key in the store, folder name)
        let mut dirs: Vec<(String, String)> = Vec::new();
        for i in 0..self.store.n_items() {
            if let Some(item) = self.store.item(i) {
                let fo = item.downcast_ref::<FileObject>().unwrap();
                if fo.is_directory() {
                    dirs.push((fo.path(), fo.name()));
                }
            }
        }

        if dirs.is_empty() {
            return;
        }

        // Channel: thread sends (path, size) pairs, main thread polls and
        // updates store. No size means a mountpoint that isn't walked.
        let (tx, rx) = std::sync::mpsc::channel::<(String, Option<u64>)>();
        let done_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_flag_thread = done_flag.clone();

        std::thread::spawn(move || {
            // Resolve symlinks here, off the main thread, so mountpoints
            // match by path.
            let base = std::fs::canonicalize(&base).unwrap_or_else(|_| base.into());
            for (key, name) in &dirs {
                let real = base.join(name);
                let size = (!skip.contains(&real)).then(|| walk_dir_size(&real, &skip));
                let _ = tx.send((key.clone(), size));
            }
            done_flag_thread.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        // Poll for results on the main thread
        let store = self.store.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            // Build index for O(1) lookups
            let mut path_to_index: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            for i in 0..store.n_items() {
                if let Some(item) = store.item(i) {
                    let fo = item.downcast_ref::<FileObject>().unwrap();
                    if fo.is_directory() {
                        path_to_index.insert(fo.path(), i);
                    }
                }
            }

            // Drain all available results
            while let Ok((path, size)) = rx.try_recv() {
                // Try O(1) lookup first
                let idx = path_to_index.get(&path).copied().or_else(|| {
                    // Fallback: linear scan if indices shifted
                    (0..store.n_items()).find(|&i| {
                        store
                            .item(i)
                            .and_then(|item| {
                                item.downcast_ref::<FileObject>()
                                    .map(|fo| fo.path() == path)
                            })
                            .unwrap_or(false)
                    })
                });
                if let Some(i) = idx {
                    if let Some(item) = store.item(i) {
                        let fo = item.downcast_ref::<FileObject>().unwrap();
                        match size {
                            Some(size) => {
                                fo.set_size(size);
                                fo.set_size_display(crate::file_object::format_size(size));
                            }
                            None => fo.set_size_display("\u{2014}".to_string()),
                        }
                    }
                }
            }

            if done_flag.load(std::sync::atomic::Ordering::Relaxed) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Watch a local directory for changes. (Not used on FUSE mounts.)
    fn setup_monitor(&self, dir: &gio::File) {
        // Drop old monitor
        *self.monitor.borrow_mut() = None;

        match dir.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(mon) => {
                let store = self.store.clone();
                let current_path = self.current_path.clone();
                let sorter = self.sorter.clone();
                let sort_model = self.sort_model.clone();

                mon.connect_changed(move |_mon, file, _other, event| {
                    let path = current_path.borrow().clone();

                    // Get the basename of the changed file for matching
                    let changed_name = file
                        .basename()
                        .map(|b| b.to_string_lossy().to_string())
                        .unwrap_or_default();

                    // Helper: find existing item index by name
                    let find_index = |store: &gio::ListStore, name: &str| -> Option<u32> {
                        for i in 0..store.n_items() {
                            if let Some(item) = store.item(i) {
                                let fo = item.downcast_ref::<FileObject>().unwrap();
                                if fo.name() == name {
                                    return Some(i);
                                }
                            }
                        }
                        None
                    };

                    match event {
                        gio::FileMonitorEvent::Created
                            if find_index(&store, &changed_name).is_none() =>
                        {
                            if let Ok(info) = file.query_info(
                                "standard::*,time::modified",
                                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                                gio::Cancellable::NONE,
                            ) {
                                let file_obj = FileObject::from_file_info(&path, &info);
                                store.append(&file_obj);
                                sorter.changed(gtk::SorterChange::Different);
                            }
                        }
                        gio::FileMonitorEvent::Deleted => {
                            if let Some(idx) = find_index(&store, &changed_name) {
                                store.remove(idx);
                            }
                        }
                        gio::FileMonitorEvent::AttributeChanged
                        | gio::FileMonitorEvent::ChangesDoneHint => {
                            if let Some(idx) = find_index(&store, &changed_name) {
                                if let Ok(info) = file.query_info(
                                    "standard::*,time::modified",
                                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                                    gio::Cancellable::NONE,
                                ) {
                                    // Update in place, keeping the selection on it.
                                    let new_obj = FileObject::from_file_info(&path, &info);
                                    if let Some(fo) = store.item(idx).and_downcast::<FileObject>() {
                                        fo.update_from(&new_obj);
                                        if let Some(sorter) = sort_model.sorter() {
                                            sorter.changed(gtk::SorterChange::Different);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                });

                *self.monitor.borrow_mut() = Some(mon);
            }
            Err(e) => {
                log::warn!("Failed to set up file monitor: {e}");
            }
        }
    }

    pub fn toggle_hidden(&self) -> bool {
        let mut show = self.show_hidden.borrow_mut();
        *show = !*show;
        let visible = *show;
        drop(show);
        self.hidden_filter.changed(FilterChange::Different);
        visible
    }

    pub fn showing_hidden(&self) -> bool {
        *self.show_hidden.borrow()
    }

    pub fn item_count(&self) -> u32 {
        self.filter_model.n_items()
    }

    pub fn current_path(&self) -> String {
        self.current_path.borrow().clone()
    }

    pub fn is_fuse(&self) -> bool {
        self.is_fuse.get()
    }

    /// Set an external sorter (e.g. from ColumnView) to drive sorting
    pub fn set_sorter(&self, sorter: Option<&impl IsA<gtk::Sorter>>) {
        self.sort_model.set_sorter(sorter);
    }
}

/// Paths of recently used local files. The RecentManager is main-thread
/// only, so this runs there; it touches no files.
fn recent_paths() -> Vec<String> {
    gtk::RecentManager::default()
        .items()
        .iter()
        .filter(|item| item.uri().starts_with("file://"))
        .filter_map(|item| gio::File::for_uri(&item.uri()).path())
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

/// Mountpoints of FUSE and network filesystems, from `/proc/mounts`.
/// Reading them can block indefinitely (and blocks suspend while it does).
fn remote_mountpoints() -> std::collections::HashSet<std::path::PathBuf> {
    const NETWORK: &[&str] = &["nfs", "nfs4", "cifs", "smb3", "smbfs", "9p", "ceph", "afs"];
    let Ok(contents) = std::fs::read_to_string("/proc/mounts") else {
        return Default::default();
    };
    contents
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let mountpoint = parts.nth(1)?;
            let fstype = parts.next()?;
            let remote = fstype.starts_with("fuse") || NETWORK.contains(&fstype);
            remote.then(|| std::path::PathBuf::from(mountpoint.replace("\\040", " ")))
        })
        .collect()
}

fn walk_dir_size(
    dir: &std::path::Path,
    skip: &std::collections::HashSet<std::path::PathBuf>,
) -> u64 {
    walk_dir_size_impl(dir, skip, 0)
}

fn walk_dir_size_impl(
    dir: &std::path::Path,
    skip: &std::collections::HashSet<std::path::PathBuf>,
    depth: u32,
) -> u64 {
    if depth > 100 {
        return 0;
    }
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if skip.contains(&path) {
            continue;
        }
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += walk_dir_size_impl(&path, skip, depth + 1);
        }
    }
    total
}
