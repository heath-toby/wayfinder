use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{CustomFilter, CustomSorter, FilterChange, FilterListModel, SortListModel};

use crate::file_object::FileObject;
use crate::search::SearchState;

/// Check whether a path resides on a FUSE filesystem (e.g. rclone, sshfs).
/// Reads `/proc/mounts` and picks the longest matching mountpoint whose
/// filesystem type contains "fuse".
fn is_fuse_mount(path: &str) -> bool {
    crate::rclone_ops::is_fuse_path(path)
}

pub struct DirectoryModel {
    pub store: gio::ListStore,
    pub sort_model: SortListModel,
    pub hidden_filter_model: FilterListModel,
    pub filter_model: FilterListModel,
    pub sorter: CustomSorter,
    pub hidden_filter: CustomFilter,
    pub search: SearchState,
    show_hidden: Rc<RefCell<bool>>,
    current_path: RefCell<String>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    is_fuse: RefCell<bool>,
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
            current_path: RefCell::new(String::new()),
            monitor: RefCell::new(None),
            is_fuse: RefCell::new(false),
        }
    }

    /// Load recently used files from GTK's RecentManager.
    pub fn load_recent(&self) -> Result<u32, glib::Error> {
        self.store.remove_all();
        *self.current_path.borrow_mut() = "recent:///".to_string();
        *self.is_fuse.borrow_mut() = false;
        *self.monitor.borrow_mut() = None;

        let manager = gtk::RecentManager::default();
        let items = manager.items();

        let mut count = 0u32;
        for item in &items {
            let uri = item.uri();
            // Only show local files
            if !uri.starts_with("file://") {
                continue;
            }
            let path_str = uri.strip_prefix("file://").unwrap_or(&uri);
            let path = std::path::Path::new(path_str);
            if !path.exists() {
                continue;
            }

            let file = gio::File::for_path(path);
            let Ok(info) = file.query_info(
                "standard::*,time::modified",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            ) else {
                continue;
            };

            let parent = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let file_obj = FileObject::from_file_info(&parent, &info);
            self.store.append(&file_obj);
            count += 1;

            if count >= 50 {
                break;
            }
        }

        Ok(count)
    }

    pub fn load_uri(&self, uri: &str) -> Result<u32, glib::Error> {
        self.store.remove_all();
        *self.current_path.borrow_mut() = uri.to_string();
        *self.is_fuse.borrow_mut() = false;

        let file = gio::File::for_uri(uri);
        let enumerator = file.enumerate_children(
            "standard::*,time::modified",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;

        let mut count = 0u32;
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            let file_obj = FileObject::from_file_info(uri, &info);
            self.store.append(&file_obj);
            count += 1;
        }

        // No file monitor for special URIs
        *self.monitor.borrow_mut() = None;

        Ok(count)
    }

    pub fn load_directory(&self, path: &str) -> Result<u32, glib::Error> {
        self.store.remove_all();
        *self.current_path.borrow_mut() = path.to_string();
        *self.is_fuse.borrow_mut() = is_fuse_mount(path);

        let file = gio::File::for_path(path);
        let enumerator = file.enumerate_children(
            "standard::*,time::modified",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;

        let mut count = 0u32;
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            let file_obj = FileObject::from_file_info(path, &info);
            self.store.append(&file_obj);
            count += 1;
        }

        // Set up file monitor
        self.setup_monitor(&file);

        if *self.is_fuse.borrow() {
            // Skip recursive folder size calculation on FUSE mounts —
            // it would be extremely slow over the network
            for i in 0..self.store.n_items() {
                if let Some(item) = self.store.item(i) {
                    let fo = item.downcast_ref::<FileObject>().unwrap();
                    if fo.is_directory() {
                        fo.set_size_display("\u{2014}".to_string());
                    }
                }
            }
        } else {
            // Calculate folder sizes asynchronously
            self.calculate_folder_sizes();
        }

        Ok(count)
    }

    fn calculate_folder_sizes(&self) {
        // Collect directory paths that need size calculation
        let mut dir_paths: Vec<String> = Vec::new();
        for i in 0..self.store.n_items() {
            if let Some(item) = self.store.item(i) {
                let fo = item.downcast_ref::<FileObject>().unwrap();
                if fo.is_directory() {
                    dir_paths.push(fo.path());
                }
            }
        }

        if dir_paths.is_empty() {
            return;
        }

        // Channel: thread sends (path, size) pairs, main thread polls and updates store
        let (tx, rx) = std::sync::mpsc::channel::<(String, u64)>();
        let done_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_flag_thread = done_flag.clone();

        std::thread::spawn(move || {
            for dir_path in &dir_paths {
                let size = walk_dir_size(std::path::Path::new(dir_path));
                let _ = tx.send((dir_path.clone(), size));
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
                        fo.set_size(size);
                        fo.set_size_display(crate::file_object::format_size(size));
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

    /// Spawn a background thread that runs `query_info` with a 5-second
    /// timeout, extracts plain data, then posts the result back via a
    /// channel.  A short main-thread timer polls the channel and calls
    /// `on_result` when data arrives.
    fn query_info_async(
        file_path_str: String,
        on_result: impl FnOnce(crate::file_object::FileInfoData) + 'static,
    ) {
        use crate::file_object::FileInfoData;
        let (tx, rx) = std::sync::mpsc::channel::<Option<FileInfoData>>();
        std::thread::spawn(move || {
            let f = gio::File::for_path(&file_path_str);
            let cancel = gio::Cancellable::new();
            let cancel_timer = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                cancel_timer.cancel();
            });
            let result = f.query_info(
                "standard::*,time::modified",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                Some(&cancel),
            );
            let _ = tx.send(result.ok().map(|info| FileInfoData::from_file_info(&info)));
        });
        let on_result = std::cell::RefCell::new(Some(on_result));
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            match rx.try_recv() {
                Ok(Some(data)) => {
                    if let Some(cb) = on_result.borrow_mut().take() {
                        cb(data);
                    }
                    glib::ControlFlow::Break
                }
                Ok(None) => glib::ControlFlow::Break,
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => glib::ControlFlow::Break,
            }
        });
    }

    fn setup_monitor(&self, dir: &gio::File) {
        // Drop old monitor
        *self.monitor.borrow_mut() = None;

        match dir.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(mon) => {
                let store = self.store.clone();
                let current_path = self.current_path.clone();
                let sorter = self.sorter.clone();
                let on_fuse = *self.is_fuse.borrow();

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
                            if on_fuse {
                                let file_path_str = file
                                    .path()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let path = path.clone();
                                let store = store.clone();
                                let sorter = sorter.clone();
                                Self::query_info_async(file_path_str, move |data| {
                                    let file_obj = FileObject::from_data(&path, &data);
                                    store.append(&file_obj);
                                    sorter.changed(gtk::SorterChange::Different);
                                });
                            } else if let Ok(info) = file.query_info(
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
                            if on_fuse {
                                if find_index(&store, &changed_name).is_some() {
                                    let file_path_str = file
                                        .path()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let changed_name = changed_name.clone();
                                    let path = path.clone();
                                    let store = store.clone();
                                    let sorter = sorter.clone();
                                    Self::query_info_async(file_path_str, move |data| {
                                        let new_obj = FileObject::from_data(&path, &data);
                                        let idx = (0..store.n_items()).find(|&i| {
                                            store
                                                .item(i)
                                                .and_then(|item| {
                                                    item.downcast_ref::<FileObject>()
                                                        .map(|fo| fo.name() == changed_name)
                                                })
                                                .unwrap_or(false)
                                        });
                                        if let Some(idx) = idx {
                                            store.remove(idx);
                                            store.append(&new_obj);
                                            sorter.changed(gtk::SorterChange::Different);
                                        }
                                    });
                                }
                            } else if let Some(idx) = find_index(&store, &changed_name) {
                                if let Ok(info) = file.query_info(
                                    "standard::*,time::modified",
                                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                                    gio::Cancellable::NONE,
                                ) {
                                    let new_obj = FileObject::from_file_info(&path, &info);
                                    store.remove(idx);
                                    store.append(&new_obj);
                                    sorter.changed(gtk::SorterChange::Different);
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
        *self.is_fuse.borrow()
    }

    /// Set an external sorter (e.g. from ColumnView) to drive sorting
    pub fn set_sorter(&self, sorter: Option<&impl IsA<gtk::Sorter>>) {
        self.sort_model.set_sorter(sorter);
    }
}

fn walk_dir_size(dir: &std::path::Path) -> u64 {
    walk_dir_size_impl(dir, 0)
}

fn walk_dir_size_impl(dir: &std::path::Path, depth: u32) -> u64 {
    if depth > 100 {
        return 0;
    }
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += walk_dir_size_impl(&entry.path(), depth + 1);
        }
    }
    total
}
