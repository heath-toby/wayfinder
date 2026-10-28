mod loader;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
    /// in place, so the selection and focus stay where they are. A failed
    /// listing leaves the store alone (it may be a passing error).
    Diff,
    /// Like `Diff`, checking a folder shown from the cache. A failed
    /// listing means what's shown was never confirmed: the store is
    /// emptied and the error reported.
    Revalidate,
}

/// Called once a job is over: with its outcome, or `None` if a navigation
/// superseded it.
type LoadDone = Box<dyn FnOnce(Option<Result<u32, String>>)>;
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
    /// Changes the file monitor has made to the store, counted, and by
    /// path the count at its latest change there: a refresh's listing
    /// begun before that is older there than the store, and leaves it be.
    monitor_events: Rc<Cell<u64>>,
    monitor_touched: Rc<RefCell<HashMap<String, u64>>>,
    /// The monitor's count when each refresh still running began: what
    /// it touched before the oldest of them matters to none, and goes.
    refresh_starts: Rc<RefCell<Vec<u64>>>,
    /// True while a navigation is waiting with nothing to show yet.
    loading: Rc<Cell<bool>>,
    /// Called when a background refresh changed the store on its own.
    on_background_update: Rc<RefCell<Option<Callback>>>,
    /// Called with (location, error) when a background refresh found the
    /// location can no longer be listed; the store has been emptied.
    on_background_error: Rc<RefCell<Option<ErrorCallback>>>,
    /// Bumped by each listing's folder-size job, and by each navigation:
    /// older jobs see it and stop (walking a big tree takes a while).
    size_job: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Refresh threads still running — counted by the threads themselves,
    /// so one left waiting on a dead mount after the user moved on still
    /// counts. By location: a thread stuck on one dead mount mustn't stop
    /// refreshes of the others.
    refreshes_in_flight: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>,
    /// A refresh was asked for while a navigation was loading.
    refresh_after_load: Rc<Cell<bool>>,
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
            *show_hidden_ref.borrow() || !file.hidden()
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
            monitor_events: Rc::new(Cell::new(0)),
            monitor_touched: Rc::new(RefCell::new(HashMap::new())),
            refresh_starts: Rc::new(RefCell::new(Vec::new())),
            loading: Rc::new(Cell::new(false)),
            on_background_update: Rc::new(RefCell::new(None)),
            on_background_error: Rc::new(RefCell::new(None)),
            refreshes_in_flight: Default::default(),
            size_job: Default::default(),
            refresh_after_load: Default::default(),
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
            unless_superseded(on_done),
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
            unless_superseded(on_done),
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
            // Show the folder as it was last seen, then check it quietly —
            // as it is: if it has gone, that's an error to report (and the
            // offline copy to show), not its parent to switch to unseen.
            let job = {
                let path = path.to_string();
                move || loader::list_dir(&path, false)
            };
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
                Apply::Revalidate,
                Box::new(move |outcome| {
                    // Superseded, or failed (reported as an error instead).
                    if !matches!(outcome, Some(Ok(_))) {
                        return;
                    }
                    if let Some(cb) = model.on_background_update.borrow().as_ref() {
                        cb();
                    }
                }),
            );
            return;
        }

        self.store.remove_all();
        self.run(gen, job, Apply::Replace, unless_superseded(on_done));
    }

    /// Like `refresh`, but does nothing while another refresh is still
    /// waiting — so checking for changes each time the window is focused
    /// can't pile up threads on a mount that has stopped answering.
    pub fn refresh_if_idle(&self, on_done: impl FnOnce() + 'static) {
        let busy = self
            .refreshes_in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.current_path())
            .is_some_and(|n| *n > 0);
        if !busy {
            self.refresh(move |_| on_done());
        }
    }

    /// List the current location again and update the store in place,
    /// keeping the selection. `on_done` always runs, told whether the store
    /// now shows a fresh listing: `false` if listing failed (the store is
    /// left as it was), or a navigation was under way (its listing will be
    /// fresh) or happened meanwhile.
    pub fn refresh(&self, on_done: impl FnOnce(bool) + 'static) {
        if self.loading.get() {
            // The navigation's listing may have begun before whatever
            // this refresh is for: list again once it's in.
            self.refresh_after_load.set(true);
            on_done(false);
            return;
        }
        let location = self.current_path();
        let on_done: LoadDone = Box::new(move |outcome| on_done(matches!(outcome, Some(Ok(_)))));
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

    /// If the navigation just started to `location` is still waiting after
    /// a moment, call `notify` with a message saying so (to show and
    /// announce) and how to back out.
    pub fn notify_if_waiting(&self, location: &str, notify: impl FnOnce(String) + 'static) {
        if !self.is_loading() {
            return;
        }
        let generation = self.generation();
        let model = self.clone();
        let location = location.to_string();
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if model.generation() != generation || !model.is_loading() {
                return;
            }
            let name = display_name(&location);
            notify(match crate::rclone_ops::fuse_daemon_name(&location) {
                Some(daemon) => {
                    format!("Waiting for {daemon} to open {name}. Press Alt+Left to go back.")
                }
                None => format!("Waiting for {name} to open. Press Alt+Left to go back."),
            });
        });
    }

    /// Drop the cached listing of `path` (e.g. files were removed from it
    /// while the user was elsewhere), so it isn't shown out of date.
    pub fn forget_cached(&self, path: &str) {
        loader::forget(path);
    }

    /// Stop waiting for the load under way (its window is closing): its
    /// listing will be dropped when it comes, and nothing polls for it
    /// meanwhile.
    pub fn abandon_loading(&self) {
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
    }

    /// Drop the cached listings of `path` and every folder inside it (it
    /// was removed or moved away).
    pub fn forget_cached_under(&self, path: &str) {
        loader::forget_under(path);
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
        self.refresh_after_load.set(false);
        *self.current_path.borrow_mut() = location.to_string();
        self.is_fuse.set(is_fuse);
        *self.monitor.borrow_mut() = None;
        self.monitor_touched.borrow_mut().clear();
        self.refresh_starts.borrow_mut().clear();
        // Sizes of the folder left behind aren't wanted any more (the new
        // one may be on a mount, or have no folders, and start no job).
        self.size_job
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let since = loader::changes();
        let events = self.monitor_events.get();
        if apply != Apply::Replace {
            self.refresh_starts.borrow_mut().push(events);
        }
        let rx = if apply == Apply::Replace {
            loader::spawn(job)
        } else {
            let in_flight = self.refreshes_in_flight.clone();
            let location = self.current_path();
            *in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(location.clone())
                .or_default() += 1;
            loader::spawn(move || {
                let result = job();
                let mut counts = in_flight.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(n) = counts.get_mut(&location) {
                    *n -= 1;
                    if *n == 0 {
                        counts.remove(&location);
                    }
                }
                result
            })
        };
        if apply == Apply::Replace {
            match rx.recv_timeout(SYNC_WAIT) {
                Ok(result) => return self.finish(gen, seq, since, events, result, apply, on_done),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let failed = Err("listing failed".into());
                    return self.finish(gen, seq, since, events, failed, apply, on_done);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => self.loading.set(true),
            }
        }

        let model = self.clone();
        let mut on_done = Some(on_done);
        glib::timeout_add_local(Duration::from_millis(30), move || {
            let result = if model.generation.get() != gen {
                // Superseded. The thread may still be waiting on the mount;
                // its result will be dropped with `rx`.
                if let Some(on_done) = on_done.take() {
                    on_done(None);
                }
                None
            } else {
                match rx.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err("listing failed".into())),
                }
            };
            if let (Some(result), Some(on_done)) = (result, on_done.take()) {
                model.finish(gen, seq, since, events, result, apply, on_done);
            }
            glib::ControlFlow::Break
        });
    }

    /// Apply a finished listing job, begun at the cache mark `since` and
    /// the monitor's count `events`.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        gen: u64,
        seq: u64,
        since: u64,
        events: u64,
        result: loader::ListResult,
        apply: Apply,
        on_done: LoadDone,
    ) {
        if apply != Apply::Replace {
            self.finish_refresh(events);
        }
        if gen != self.generation.get() {
            log::debug!("dropping listing for a location left behind (generation {gen})");
            on_done(None);
            return;
        }
        if seq < self.applied_seq.get() {
            // A newer listing of this location is already applied, so the
            // store is at least as fresh as this one would make it.
            log::debug!("skipping listing overtaken by a newer one (sequence {seq})");
            on_done(Some(Ok(self.store.n_items())));
            return;
        }
        if apply == Apply::Replace {
            self.loading.set(false);
        }
        if apply != Apply::Diff || result.is_ok() {
            self.applied_seq.set(seq);
        }
        let listing = match result {
            Ok(listing) => listing,
            Err(e) if apply == Apply::Diff => {
                // Nothing applied (it may be a passing error), so a listing
                // still to come — a cached folder's check — isn't overtaken.
                log::warn!("refreshing {} failed: {e}", self.current_path());
                on_done(Some(Err(e)));
                return;
            }
            Err(e) => {
                if apply == Apply::Revalidate {
                    // The cached listing on screen can't be confirmed
                    // (deleted, mount gone): stop showing what it held.
                    let location = self.current_path();
                    self.store.remove_all();
                    loader::forget(&location);
                    if let Some(cb) = self.on_background_error.borrow().as_ref() {
                        cb(location, e.clone());
                    }
                }
                on_done(Some(Err(e)));
                return;
            }
        };

        log::debug!("listed {}: {} entries", listing.path, listing.entries.len());
        *self.current_path.borrow_mut() = listing.path.clone();
        self.is_fuse.set(listing.is_fuse);
        if listing.is_fuse {
            loader::store(&listing, since);
        }
        match apply {
            Apply::Replace => self.replace_all(&listing),
            Apply::Diff | Apply::Revalidate => {
                // What the monitor changed since this was listed is newer
                // here than the listing: applied, it would bring back what
                // was deleted meanwhile, drop what was made, or undo an
                // update.
                let newer: std::collections::HashSet<String> = self
                    .monitor_touched
                    .borrow()
                    .iter()
                    .filter(|(_, &n)| n > events)
                    .map(|(path, _)| path.clone())
                    .collect();
                self.apply_diff(&listing, &newer);
            }
        }

        // (By the path listed too: one through a link kept on a mount is
        // looked up on the mount.)
        if listing.path.starts_with('/')
            && !listing.is_fuse
            && !crate::rclone_ops::is_fuse_path(&listing.path)
        {
            // FUSE folders get neither: a monitor's path lookup would touch
            // the mount on this thread (the window refreshes them when it
            // regains focus instead), and sizes would mean walking the
            // remote, so they keep showing a dash.
            // (A refresh keeps the monitor it has: made anew, the old one's
            // waiting events — a file written meanwhile — would be lost.)
            if apply == Apply::Replace || self.monitor.borrow().is_none() {
                self.setup_monitor(&gio::File::for_path(&listing.path));
            }
            self.calculate_folder_sizes();
        }
        on_done(Some(Ok(self.store.n_items())));
        if apply == Apply::Replace && self.refresh_after_load.replace(false) {
            // Its changes come in the background, like a cached folder's
            // check: tell the window, so its item count follows.
            let model = self.clone();
            self.refresh(move |listed| {
                if listed {
                    if let Some(cb) = model.on_background_update.borrow().as_ref() {
                        cb();
                    }
                }
            });
        }
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

    /// A refresh begun at the monitor's count `events` is over (about to be
    /// applied, or dropped): forget what the monitor touched before it and
    /// every refresh still running began.
    fn finish_refresh(&self, events: u64) {
        let mut starts = self.refresh_starts.borrow_mut();
        if let Some(i) = starts.iter().position(|&s| s == events) {
            starts.remove(i);
        }
        // (Newer than this one's start too: it's about to be applied.)
        let oldest = starts.iter().copied().fold(events, u64::min);
        self.monitor_touched.borrow_mut().retain(|_, n| *n > oldest);
    }

    /// `keep`: paths the store has newer than the listing, left as they are.
    fn apply_diff(&self, listing: &Listing, keep: &std::collections::HashSet<String>) {
        // Keyed by full path: Recent can hold equal names from different folders.
        let mut fresh: std::collections::HashMap<String, FileObject> = listing
            .entries
            .iter()
            .map(|(parent, data)| {
                let fo = FileObject::from_data(parent, data);
                (fo.path(), fo)
            })
            .collect();
        fresh.retain(|path, _| !keep.contains(path));
        let mut updated: Vec<FileObject> = Vec::new();
        for i in (0..self.store.n_items()).rev() {
            let Some(fo) = self.store.item(i).and_downcast::<FileObject>() else {
                continue;
            };
            if keep.contains(&fo.path()) {
                continue;
            }
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
                        updated.push(fo);
                    }
                }
            }
        }
        let added: Vec<glib::Object> = fresh.into_values().map(|fo| fo.upcast()).collect();
        self.store.splice(self.store.n_items(), 0, &added);
        resort_if_needed(&self.sort_model, &updated);
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

        let token = self
            .size_job
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let job = SizeJob(self.size_job.clone(), token);
        spawn_size_job(&self.store, &self.sort_model, base, dirs, skip, job);
    }

    /// Watch a local directory for changes. (Not used on FUSE mounts.)
    fn setup_monitor(&self, dir: &gio::File) {
        // Drop old monitor
        *self.monitor.borrow_mut() = None;

        match dir.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(mon) => {
                let store = self.store.clone();
                let current_path = self.current_path.clone();
                let sort_model = self.sort_model.clone();
                let size_job = self.size_job.clone();
                let monitor_events = self.monitor_events.clone();
                let monitor_touched = self.monitor_touched.clone();
                let touch = move |path: String| {
                    let n = monitor_events.get() + 1;
                    monitor_events.set(n);
                    monitor_touched.borrow_mut().insert(path, n);
                };

                mon.connect_changed(move |_mon, file, _other, event| {
                    let path = current_path.borrow().clone();

                    // Only the folder's entries: an event about the folder
                    // itself would hit a child of the same name.
                    if file.parent().and_then(|p| p.path()) != Some(std::path::PathBuf::from(&path))
                    {
                        return;
                    }

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
                                // (The sort model puts it in its place:
                                // re-sorting everything would make Orca
                                // lose its place.)
                                store.append(&file_obj);
                                touch(file_obj.path());
                                // It says "Calculating...": do so.
                                if file_obj.is_directory() {
                                    // Part of the current listing's job:
                                    // stopped by the next one.
                                    let token = size_job.load(std::sync::atomic::Ordering::SeqCst);
                                    spawn_size_job(
                                        &store,
                                        &sort_model,
                                        path.clone(),
                                        vec![(file_obj.path(), file_obj.name())],
                                        remote_mountpoints(),
                                        SizeJob(size_job.clone(), token),
                                    );
                                }
                            }
                        }
                        gio::FileMonitorEvent::Deleted => {
                            if let Some(idx) = find_index(&store, &changed_name) {
                                if let Some(fo) = store.item(idx).and_downcast::<FileObject>() {
                                    touch(fo.path());
                                }
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
                                        touch(fo.path());
                                        resort_if_needed(&sort_model, &[fo]);
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

/// Work out the sizes of the folders `dirs` ((key in the store, name in
/// `base`)) on a worker thread, and show each as it comes in.
/// A folder-size job's place in line: it stops once a newer one starts.
struct SizeJob(std::sync::Arc<std::sync::atomic::AtomicU64>, u64);

impl SizeJob {
    fn superseded(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst) != self.1
    }
}

fn spawn_size_job(
    store: &gio::ListStore,
    sort_model: &SortListModel,
    base: String,
    dirs: Vec<(String, String)>,
    skip: std::collections::HashSet<std::path::PathBuf>,
    job: SizeJob,
) {
    // Channel: thread sends (path, size) pairs, main thread polls and
    // updates store. No size means a folder that isn't walked.
    let (tx, rx) = std::sync::mpsc::channel::<(String, Option<u64>)>();
    std::thread::spawn(move || {
        // Resolve symlinks here, off the main thread, so mountpoints
        // match by path.
        let base = std::fs::canonicalize(&base).unwrap_or_else(|_| base.into());
        // Inside a network mount (NFS, CIFS…), every walk would go over the
        // network: don't (the same reason FUSE folders aren't walked).
        let on_network = skip.iter().any(|m| base.starts_with(m));
        // Nor into anything mounted (/proc, /sys, a disk): its size isn't
        // the folder's. By mountpoint, not device number — btrfs
        // subvolumes and overlay layers have their own, but are the folder.
        let mounts = all_mountpoints();
        for (key, name) in &dirs {
            if job.superseded() {
                return;
            }
            let real = base.join(name);
            // A link to a folder isn't walked: it may lead anywhere — onto
            // a mount, or round in a loop.
            let walkable = !on_network
                && !mounts.contains(&real)
                && real
                    .symlink_metadata()
                    .is_ok_and(|m| !m.file_type().is_symlink());
            let size = walkable.then(|| walk_dir_size(&real, &mounts, &job));
            if job.superseded() {
                return;
            }
            let _ = tx.send((key.clone(), size));
        }
    });

    // Poll for results on the main thread, until the thread is done and
    // every result is in (the channel disconnects only after the last).
    let store = store.clone();
    let sort_model = sort_model.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let mut results = Vec::new();
        let finished = loop {
            match rx.try_recv() {
                Ok(result) => results.push(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        let mut updated = Vec::new();
        if !results.is_empty() {
            // The folders shown, by path: looked up once per batch, not
            // once per result (thousands of folders would freeze it).
            let folders: HashMap<String, FileObject> = (0..store.n_items())
                .filter_map(|i| store.item(i).and_downcast::<FileObject>())
                .filter(|fo| fo.is_directory())
                .map(|fo| (fo.path(), fo))
                .collect();
            for (path, size) in results {
                if let Some(fo) = folders.get(&path).cloned() {
                    match size {
                        Some(size) => {
                            fo.set_size(size);
                            fo.set_size_display(crate::file_object::format_size(size));
                        }
                        None => fo.set_size_display("\u{2014}".to_string()),
                    }
                    updated.push(fo);
                }
            }
        }
        // Sorted by size, folders move to their places as sizes arrive.
        resort_if_needed(&sort_model, &updated);
        if finished {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Re-apply the active sort if items changed in place are now out of
/// order (the sort model places new items, but doesn't notice changes).
/// Only then: re-sorting replaces every row, which can make Orca lose or
/// repeat its place.
fn resort_if_needed(sort_model: &SortListModel, changed: &[FileObject]) {
    let Some(sorter) = sort_model.sorter() else {
        return;
    };
    if changed.is_empty() {
        return;
    }
    let changed: std::collections::HashSet<&FileObject> = changed.iter().collect();
    let n = sort_model.n_items();
    // One pass over the list for all of them.
    let out_of_order = (0..n).any(|pos| {
        let Some(fo) = sort_model.item(pos).and_downcast::<FileObject>() else {
            return false;
        };
        if !changed.contains(&fo) {
            return false;
        }
        let fo = &fo;
        let before = pos
            .checked_sub(1)
            .and_then(|i| sort_model.item(i))
            .is_some_and(|prev| sorter.compare(&prev, fo) == gtk::Ordering::Larger);
        let after = sort_model
            .item(pos + 1)
            .is_some_and(|next| sorter.compare(fo, &next) == gtk::Ordering::Larger);
        before || after
    });
    if out_of_order {
        sorter.changed(gtk::SorterChange::Different);
    }
}

/// Look up one file — off the main thread, it may be on a slow mount —
/// without sniffing its contents on a FUSE mount. Returns its folder and
/// details.
pub fn query_file(path: &str) -> Option<(String, crate::file_object::FileInfoData)> {
    let info = gio::File::for_path(path)
        .query_info(
            // Where it really is: through a link into a mount, it's on the
            // mount (and mustn't have its contents sniffed).
            loader::attrs_for(crate::rclone_ops::is_fuse_path(
                &crate::rclone_ops::resolve_links_outside_mounts(std::path::Path::new(path))
                    .to_string_lossy(),
            )),
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let parent = std::path::Path::new(path)
        .parent()?
        .to_string_lossy()
        .to_string();
    Some((
        parent,
        crate::file_object::FileInfoData::from_file_info(&info),
    ))
}

/// Paths of recently used local files. The RecentManager is main-thread
/// only, so this runs there; it touches no files.
fn recent_paths() -> Vec<String> {
    // Most recently used first: the list is cut to the newest ones, and
    // the manager's own order is the file's, not by use.
    let mut items = gtk::RecentManager::default().items();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));
    items
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
            remote
                .then(|| std::path::PathBuf::from(crate::rclone_ops::decode_mount_path(mountpoint)))
        })
        .collect()
}

fn walk_dir_size(
    dir: &std::path::Path,
    skip: &std::collections::HashSet<std::path::PathBuf>,
    job: &SizeJob,
) -> u64 {
    walk_dir_size_impl(dir, skip, job, 0)
}

/// Every mountpoint, from /proc/mounts.
fn all_mountpoints() -> std::collections::HashSet<std::path::PathBuf> {
    std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|m| std::path::PathBuf::from(crate::rclone_ops::decode_mount_path(m)))
        .collect()
}

/// Doesn't go into `skip` (mountpoints).
fn walk_dir_size_impl(
    dir: &std::path::Path,
    skip: &std::collections::HashSet<std::path::PathBuf>,
    job: &SizeJob,
    depth: u32,
) -> u64 {
    if depth > 100 || job.superseded() {
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
            total += walk_dir_size_impl(&path, skip, job, depth + 1);
        }
    }
    total
}

/// A navigation's `on_done`, which doesn't run if it's superseded.
fn unless_superseded(on_done: impl FnOnce(Result<u32, String>) + 'static) -> LoadDone {
    Box::new(move |outcome| {
        if let Some(result) = outcome {
            on_done(result);
        }
    })
}

/// How to name a location when speaking of it: a folder by its own name.
pub fn display_name(location: &str) -> String {
    match location {
        "recent:///" => "Recent Files".to_string(),
        "trash:///" => "the Bin".to_string(),
        l => match l.split_once("://") {
            // Another GIO location: its last name, or else its scheme.
            Some((scheme, rest)) => rest
                .trim_end_matches('/')
                .rsplit('/')
                .find(|s| !s.is_empty())
                .map(|name| {
                    glib::Uri::unescape_string(name, None::<&str>)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| name.to_string())
                })
                .unwrap_or_else(|| scheme.to_string()),
            None => std::path::Path::new(l)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| l.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::display_name;

    #[test]
    fn folders_are_named_by_their_own_name() {
        assert_eq!(display_name("/home/toby/Music"), "Music");
        assert_eq!(display_name("/home/toby/Music/"), "Music");
        assert_eq!(display_name("/"), "/");
    }

    #[test]
    fn special_locations_get_their_names() {
        assert_eq!(display_name("trash:///"), "the Bin");
        assert_eq!(display_name("recent:///"), "Recent Files");
    }

    #[test]
    fn other_uris_are_named_by_their_last_part() {
        assert_eq!(display_name("trash:///Old%20Folder"), "Old Folder");
        assert_eq!(display_name("smb://server/share/Reports/"), "Reports");
        assert_eq!(display_name("sftp://server/"), "server");
        assert_eq!(display_name("network:///"), "network");
    }
}
