use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::clipboard::ClipboardState;
use wayfinder::file_model::DirectoryModel;
use wayfinder::file_object::FileObject;
use wayfinder::navigation::NavigationState;
use wayfinder::sidebar::WayfinderSidebar;
use wayfinder::views::{WayfinderGridView, WayfinderListView};

const COLUMN_NAMES: &[&str] = &["Name", "Size", "Date Modified", "Kind"];

pub struct SelectionState {
    pub selected: std::collections::HashSet<String>, // paths of selected files
    pub range_anchor: Option<u32>,                   // position where Shift+Space started
}

impl SelectionState {
    pub fn new() -> Self {
        Self {
            selected: std::collections::HashSet::new(),
            range_anchor: None,
        }
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }

    pub fn is_selected(&self, path: &str) -> bool {
        self.selected.contains(path)
    }

    pub fn toggle(&mut self, path: &str) -> bool {
        if self.selected.contains(path) {
            self.selected.remove(path);
            false
        } else {
            self.selected.insert(path.to_string());
            true
        }
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.range_anchor = None;
    }
}

pub struct WayfinderWindowInner {
    pub model: DirectoryModel,
    pub list_view: WayfinderListView,
    pub grid_view: WayfinderGridView,
    pub selection: gtk::SingleSelection,
    pub nav: RefCell<NavigationState>,
    pub location_entry: gtk::Entry,
    pub status_label: gtk::Label,
    pub back_button: gtk::Button,
    pub forward_button: gtk::Button,
    pub up_button: gtk::Button,
    pub current_column: Cell<usize>,
    pub view_stack: gtk::Stack,
    pub sidebar: WayfinderSidebar,
    pub sidebar_revealer: gtk::Revealer,
    pub sidebar_toggle: gtk::ToggleButton,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub clipboard: RefCell<Option<ClipboardState>>,
    /// Guard so a second Ctrl+V (or drag-drop) can't fire while a paste is
    /// already running. Cleared by `paste_from`'s reload callback.
    pub pasting: Cell<bool>,
    /// Showing a folder's offline copy because its mount is unreachable:
    /// what's listed lives in the cache, not on the drive.
    pub offline_view: Cell<bool>,
    /// A restore from the Bin is under way.
    pub restoring: Cell<bool>,
    /// This window's place in the session (see join_session).
    pub session_id: Cell<u64>,
    pub current_view: Cell<ViewMode>,
    pub file_selection: RefCell<SelectionState>,
    pub type_ahead_buffer: Rc<RefCell<String>>,
    pub type_ahead_generation: Rc<Cell<u32>>,
    pub last_trashed: RefCell<Vec<String>>,
    /// Paths whose move to Bin is still waiting on the filesystem.
    pub trashing: RefCell<std::collections::HashSet<String>>,
    /// Remembers the last-selected filename per directory so that
    /// navigating back into a folder restores the previous selection.
    pub last_position: RefCell<std::collections::HashMap<String, String>>,
    pub breadcrumb_box: gtk::Box,
    pub breadcrumb_scroll: gtk::ScrolledWindow,
    pub breadcrumb_toggle: gtk::ToggleButton,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
}

impl Default for WayfinderWindowInner {
    fn default() -> Self {
        let model = DirectoryModel::new();
        let selection = gtk::SingleSelection::new(Some(model.filter_model.clone()));
        selection.set_autoselect(true);

        let list_view = WayfinderListView::new();
        list_view.set_model(&selection);

        // Grid view starts without a model — only the active view gets one
        let grid_view = WayfinderGridView::new();

        let initial_dir = wayfinder::state::load_last_directory()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| "/".into());
        let nav = RefCell::new(NavigationState::new(initial_dir));

        let location_entry = gtk::Entry::builder()
            .hexpand(true)
            .editable(false)
            .can_focus(true)
            .placeholder_text("Path")
            .build();
        location_entry.update_property(&[
            gtk::accessible::Property::Label("Location"),
            gtk::accessible::Property::Description(
                "Current directory path. Press Ctrl+L to navigate.",
            ),
        ]);

        let status_label = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(6)
            .margin_end(6)
            .margin_top(2)
            .margin_bottom(2)
            .build();
        status_label.update_property(&[gtk::accessible::Property::Label("Status bar")]);

        let back_button = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Go back (Alt+Left)")
            .sensitive(false)
            .build();
        back_button.update_property(&[gtk::accessible::Property::Label("Go back")]);

        let forward_button = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Go forward (Alt+Right)")
            .sensitive(false)
            .build();
        forward_button.update_property(&[gtk::accessible::Property::Label("Go forward")]);

        let up_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Go to parent directory (Alt+Up)")
            .build();
        up_button.update_property(&[gtk::accessible::Property::Label("Go to parent directory")]);

        // View stack
        let view_stack = gtk::Stack::new();
        view_stack.set_transition_type(gtk::StackTransitionType::None);
        view_stack.add_named(grid_view.widget(), Some("grid"));
        view_stack.add_named(list_view.widget(), Some("list"));

        // Sidebar
        let sidebar = WayfinderSidebar::new();
        let sidebar_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .reveal_child(true)
            .child(sidebar.widget())
            .build();

        // Search
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Filter files..."));
        search_entry.update_property(&[
            gtk::accessible::Property::Label("Filter files"),
            gtk::accessible::Property::Description("Type to filter files in the current directory"),
        ]);

        let search_bar = gtk::SearchBar::builder()
            .show_close_button(true)
            .child(&search_entry)
            .build();
        search_bar.connect_entry(&search_entry);

        Self {
            model,
            list_view,
            grid_view,
            selection,
            nav,
            location_entry,
            status_label,
            back_button,
            forward_button,
            up_button,
            current_column: Cell::new(0),
            view_stack,
            sidebar,
            sidebar_revealer,
            sidebar_toggle: gtk::ToggleButton::builder()
                .icon_name("sidebar-show-symbolic")
                .tooltip_text("Toggle sidebar (F3)")
                .build(),
            search_bar,
            search_entry,
            clipboard: RefCell::new(None),
            pasting: Cell::new(false),
            offline_view: Cell::new(false),
            restoring: Cell::new(false),
            session_id: Cell::new(0),
            current_view: Cell::new(ViewMode::List),
            file_selection: RefCell::new(SelectionState::new()),
            type_ahead_buffer: Rc::new(RefCell::new(String::new())),
            type_ahead_generation: Rc::new(Cell::new(0)),
            last_trashed: RefCell::new(Vec::new()),
            trashing: RefCell::new(std::collections::HashSet::new()),
            last_position: RefCell::new(std::collections::HashMap::new()),
            breadcrumb_box: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            breadcrumb_scroll: gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .hexpand(true)
                .build(),
            breadcrumb_toggle: gtk::ToggleButton::builder()
                .icon_name("view-wrapped-symbolic")
                .tooltip_text("Toggle breadcrumb bar (F4)")
                .build(),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WayfinderWindowInner {
    const NAME: &'static str = "WayfinderWindow";
    type Type = super::WayfinderWindow;
    type ParentType = gtk::ApplicationWindow;
}

impl ObjectImpl for WayfinderWindowInner {
    fn constructed(&self) {
        self.parent_constructed();
        self.obj().join_session();
        let window = self.obj();

        window.set_title(Some("Wayfinder"));
        let (w, h) = wayfinder::state::load_window_size();
        window.set_default_size(w, h);

        // Header bar
        let header = gtk::HeaderBar::new();
        let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        nav_box.add_css_class("linked");
        nav_box.append(&self.back_button);
        nav_box.append(&self.forward_button);
        header.pack_start(&nav_box);
        header.pack_start(&self.up_button);
        // Breadcrumb path bar as the header title widget
        self.breadcrumb_scroll.set_child(Some(&self.breadcrumb_box));
        self.breadcrumb_box
            .update_property(&[gtk::accessible::Property::Label("Path breadcrumb")]);
        header.set_title_widget(Some(&self.breadcrumb_scroll));

        // Sidebar toggle button — restore saved state
        let sidebar_visible = wayfinder::state::load_sidebar_visible();
        self.sidebar_revealer.set_reveal_child(sidebar_visible);
        self.sidebar_toggle.set_active(sidebar_visible);
        self.sidebar_toggle
            .update_property(&[gtk::accessible::Property::Label("Toggle sidebar")]);
        let revealer = self.sidebar_revealer.clone();
        let w = window.clone();
        self.sidebar_toggle.connect_toggled(move |btn| {
            let visible = btn.is_active();
            revealer.set_reveal_child(visible);
            wayfinder::state::save_sidebar_visible(visible);
            let msg = if visible {
                "Sidebar shown"
            } else {
                "Sidebar hidden"
            };
            w.announce(msg, gtk::AccessibleAnnouncementPriority::Medium);
        });
        header.pack_end(&self.sidebar_toggle);

        // Breadcrumb toggle button
        let breadcrumb_visible = wayfinder::state::load_breadcrumb_visible();
        self.breadcrumb_scroll.set_visible(breadcrumb_visible);
        self.breadcrumb_toggle.set_active(breadcrumb_visible);
        self.breadcrumb_toggle
            .update_property(&[gtk::accessible::Property::Label("Toggle breadcrumb bar")]);
        let scroll = self.breadcrumb_scroll.clone();
        let w = window.clone();
        self.breadcrumb_toggle.connect_toggled(move |btn| {
            let visible = btn.is_active();
            scroll.set_visible(visible);
            wayfinder::state::save_breadcrumb_visible(visible);
            let msg = if visible {
                "Breadcrumb bar shown"
            } else {
                "Breadcrumb bar hidden"
            };
            w.announce(msg, gtk::AccessibleAnnouncementPriority::Medium);
        });
        header.pack_end(&self.breadcrumb_toggle);

        window.set_titlebar(Some(&header));

        // Content area with sidebar
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_hexpand(true);
        content.append(&self.search_bar);
        content.append(&self.view_stack);
        content.append(&self.status_label);

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&self.sidebar_revealer)
            .end_child(&content)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .position(180)
            .build();

        window.set_child(Some(&paned));

        // Restore saved view mode
        let saved_view = wayfinder::state::load_view_mode();
        let initial_mode = match saved_view.as_deref() {
            Some("grid") => {
                // Detach list, attach grid
                self.list_view
                    .column_view()
                    .set_model(gtk::SelectionModel::NONE);
                self.grid_view.set_model(&self.selection);
                self.view_stack.set_visible_child_name("grid");
                ViewMode::Grid
            }
            _ => {
                self.view_stack.set_visible_child_name("list");
                ViewMode::List
            }
        };
        self.current_view.set(initial_mode);

        // Restore hidden files setting
        if wayfinder::state::load_show_hidden() {
            self.model.toggle_hidden();
        }

        // Register actions
        self.setup_actions();

        // Register keyboard shortcuts
        crate::shortcuts::register_shortcuts(&window);

        // Zoom: one setting for the whole display (CSS applies to every
        // window), set up by the first window.
        super::init_zoom(&window);

        // Connect list view activation (Enter / double-click)
        self.connect_activation();

        // Connect grid view activation
        self.connect_grid_activation();

        // Ctrl+L action opens location dialog
        self.setup_location_dialog();

        // Connect nav buttons
        self.connect_nav_buttons();

        // Connect column navigation (Left/Right) and Tab escape on the list view
        self.connect_list_key_navigation();

        // Connect grid key navigation
        self.connect_grid_key_navigation();

        // Connect search
        self.connect_search();

        // Connect sidebar
        self.connect_sidebar();

        // Register context menu actions (Open, Open With, Properties)
        crate::context_menu::register_open_with_actions(&window);

        // Connect right-click and Shift+F10 for context menu
        self.connect_context_menu();

        // Connect drop targets for drag & drop
        self.connect_drop_targets();

        // When the window regains focus (e.g. returning from an external app),
        // restore focus to the selected file item
        let w = window.clone();
        window.connect_is_active_notify(move |win| {
            if win.is_active() {
                // FUSE folders have no file monitor (setting one up would
                // touch the mount on this thread, and remote changes never
                // reach it anyway): check for changes whenever the user
                // comes back to the window instead.
                let model = &w.imp().model;
                if model.is_fuse() && !model.is_loading() {
                    let w = w.clone();
                    model.refresh_if_idle(move || w.update_status());
                }
                let win2 = w.clone();
                glib::idle_add_local_once(move || {
                    win2.restore_focus_to_selected();
                });
            }
        });

        // Restore saved sort state and wire ColumnView's sorter to the model
        let (sort_col_idx, sort_asc) = wayfinder::state::load_sort_state();
        let columns = self.list_view.column_view().columns();
        let sort_order = if sort_asc {
            gtk::SortType::Ascending
        } else {
            gtk::SortType::Descending
        };
        if let Some(col) = columns
            .item(sort_col_idx)
            .and_downcast::<gtk::ColumnViewColumn>()
        {
            self.list_view
                .column_view()
                .sort_by_column(Some(&col), sort_order);
        } else if let Some(first_col) = columns.item(0).and_downcast::<gtk::ColumnViewColumn>() {
            self.list_view
                .column_view()
                .sort_by_column(Some(&first_col), gtk::SortType::Ascending);
        }
        if let Some(cv_sorter) = self.list_view.column_view().sorter() {
            self.model.set_sorter(Some(&cv_sorter));
        }

        // Load the initial directory once construction is done — unless
        // something navigated first (session restore, a folder passed on the
        // command line), so a slow mount isn't listed twice at startup.
        let initial_path = self.nav.borrow().current().to_string_lossy().to_string();
        let w = window.clone();
        glib::idle_add_local_once(move || {
            if w.imp().model.current_path().is_empty() {
                w.navigate_to_path(&initial_path);
            }
        });

        // Opening a folder in a FUSE folder is instant if its listing was
        // fetched while the selection rested on it.
        // (Weak: the window owns the selection, so a strong reference here
        // would keep a closed window alive.)
        let w = window.downgrade();
        let prefetch_token = Rc::new(Cell::new(0u64));
        self.selection.connect_selected_item_notify(move |sel| {
            let token = prefetch_token.get() + 1;
            prefetch_token.set(token);
            let Some(w) = w.upgrade() else {
                return;
            };
            let Some(file) = sel.selected_item().and_downcast::<FileObject>() else {
                return;
            };
            if !file.is_directory() || !w.imp().model.is_fuse() {
                return;
            }
            let w = w.clone();
            let prefetch_token = prefetch_token.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                if prefetch_token.get() == token {
                    w.imp().model.prefetch(&file.path());
                }
            });
        });

        // A cached FUSE folder is shown at once and checked in the
        // background; keep the item count right if the check changed it.
        // (Weak: the model holds these, and the window holds the model.)
        let w = window.downgrade();
        self.model.set_on_background_update(move || {
            if let Some(w) = w.upgrade() {
                w.update_status();
            }
        });

        // A background check found the folder can't be listed any more
        // (deleted, mount gone): report it as a failed load, which also
        // falls back to the offline copy where there is one.
        let w = window.downgrade();
        self.model.set_on_background_error(move |location, e| {
            let Some(w) = w.upgrade() else {
                return;
            };
            if location.starts_with('/') {
                w.finish_open_directory(&location, Err(e));
            } else {
                // Bin or Recent Files.
                log::error!("Failed to refresh {location}: {e}");
                w.update_status();
                w.announce(
                    &format!("Error: could not refresh: {e}"),
                    AccessibleAnnouncementPriority::High,
                );
            }
        });
    }
}

impl Drop for WayfinderWindowInner {
    fn drop(&mut self) {
        // Seen with RUST_LOG=debug once a closed window is really freed;
        // if it never appears, something still holds the window.
        log::debug!("window freed");
    }
}

impl WidgetImpl for WayfinderWindowInner {}
impl WindowImpl for WayfinderWindowInner {
    fn close_request(&self) -> glib::Propagation {
        let window = self.obj();
        let (w, h) = window.default_size();
        wayfinder::state::save_window_size(w, h);
        // A folder still opening on a stuck mount mustn't keep the closed
        // window alive, nor finish opening (and be saved as the last
        // folder) if the mount answers later.
        self.model.abandon_loading();
        self.parent_close_request()
    }
}
impl ApplicationWindowImpl for WayfinderWindowInner {}

impl WayfinderWindowInner {
    fn setup_actions(&self) {
        let window = self.obj();

        // Navigation actions
        let w = window.clone();
        let action = gio::SimpleAction::new("home", None);
        action.connect_activate(move |_, _| {
            if let Some(home) = dirs::home_dir() {
                w.navigate_to_path(&home.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("documents", None);
        action.connect_activate(move |_, _| {
            if let Some(dir) = dirs::document_dir() {
                w.navigate_to_path(&dir.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("desktop", None);
        action.connect_activate(move |_, _| {
            if let Some(dir) = dirs::desktop_dir() {
                w.navigate_to_path(&dir.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("downloads", None);
        action.connect_activate(move |_, _| {
            if let Some(dir) = dirs::download_dir() {
                w.navigate_to_path(&dir.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("root", None);
        action.connect_activate(move |_, _| {
            w.navigate_to_path("/");
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("up", None);
        action.connect_activate(move |_, _| {
            let parent = w.imp().nav.borrow().go_up();
            if let Some(parent) = parent {
                w.navigate_to_path(&parent.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("back", None);
        action.connect_activate(move |_, _| {
            let path = w.imp().nav.borrow_mut().go_back().cloned();
            if let Some(path) = path {
                w.load_directory(&path.to_string_lossy());
            }
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("forward", None);
        action.connect_activate(move |_, _| {
            let path = w.imp().nav.borrow_mut().go_forward().cloned();
            if let Some(path) = path {
                w.load_directory(&path.to_string_lossy());
            }
        });
        window.add_action(&action);

        // location-bar action is set up in setup_location_dialog()

        let w = window.clone();
        let action = gio::SimpleAction::new("toggle-hidden", None);
        action.connect_activate(move |_, _| {
            let showing = w.imp().model.toggle_hidden();
            wayfinder::state::save_show_hidden(showing);
            let msg = if showing {
                "Showing hidden files"
            } else {
                "Hidden files hidden"
            };
            w.announce(msg, AccessibleAnnouncementPriority::Medium);
            w.update_status();
        });
        window.add_action(&action);

        // View switching
        let w = window.clone();
        let action = gio::SimpleAction::new("view-grid", None);
        action.connect_activate(move |_, _| {
            w.switch_view(ViewMode::Grid);
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("view-list", None);
        action.connect_activate(move |_, _| {
            w.switch_view(ViewMode::List);
        });
        window.add_action(&action);

        // Sidebar toggle (F3) — flip the button, which triggers connect_toggled
        let toggle = self.sidebar_toggle.clone();
        let action = gio::SimpleAction::new("toggle-sidebar", None);
        action.connect_activate(move |_, _| {
            toggle.set_active(!toggle.is_active());
        });
        window.add_action(&action);

        // Breadcrumb toggle (F4)
        let toggle = self.breadcrumb_toggle.clone();
        let action = gio::SimpleAction::new("toggle-breadcrumb", None);
        action.connect_activate(move |_, _| {
            toggle.set_active(!toggle.is_active());
        });
        window.add_action(&action);

        // Search
        let search_bar = self.search_bar.clone();
        let search_entry = self.search_entry.clone();
        let action = gio::SimpleAction::new("search", None);
        action.connect_activate(move |_, _| {
            search_bar.set_search_mode(true);
            search_entry.grab_focus();
        });
        window.add_action(&action);

        // File operations
        let w = window.clone();
        let action = gio::SimpleAction::new("copy", None);
        action.connect_activate(move |_, _| {
            w.copy_selected();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("cut", None);
        action.connect_activate(move |_, _| {
            w.cut_selected();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("paste", None);
        action.connect_activate(move |_, _| {
            w.paste();
        });
        window.add_action(&action);

        // Window-local clipboard operations (Ctrl+Shift+C/X/V)
        let w = window.clone();
        let action = gio::SimpleAction::new("copy-local", None);
        action.connect_activate(move |_, _| {
            w.copy_selected_local();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("cut-local", None);
        action.connect_activate(move |_, _| {
            w.cut_selected_local();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("paste-local", None);
        action.connect_activate(move |_, _| {
            w.paste_local();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("trash", None);
        action.connect_activate(move |_, _| {
            w.trash_selected();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("delete", None);
        action.connect_activate(move |_, _| {
            w.delete_selected();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("rename", None);
        action.connect_activate(move |_, _| {
            w.rename_selected();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("new-folder", None);
        action.connect_activate(move |_, _| {
            w.create_new_folder();
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("select-all", None);
        action.connect_activate(move |_, _| {
            let imp = w.imp();
            let mut sel = imp.file_selection.borrow_mut();
            let model = &imp.model.filter_model;
            for i in 0..model.n_items() {
                if let Some(item) = model.item(i) {
                    if let Some(file) = item.downcast_ref::<FileObject>() {
                        sel.selected.insert(file.path());
                    }
                }
            }
            let count = sel.count();
            drop(sel);
            w.announce(
                &format!("{count} files selected"),
                AccessibleAnnouncementPriority::Medium,
            );
            w.update_status();
        });
        window.add_action(&action);

        // New window
        let w = window.clone();
        let action = gio::SimpleAction::new("new-window", None);
        action.connect_activate(move |_, _| {
            if let Some(app) = w.application() {
                let new_win = super::WayfinderWindow::new(&app);
                new_win.present();
            }
        });
        window.add_action(&action);

        // Shortcuts dialog
        let w = window.clone();
        let action = gio::SimpleAction::new("show-shortcuts", None);
        action.connect_activate(move |_, _| {
            w.show_shortcuts();
        });
        window.add_action(&action);

        // Bookmark current directory
        let w = window.clone();
        let action = gio::SimpleAction::new("bookmark", None);
        action.connect_activate(move |_, _| {
            let path = w.imp().model.current_path();
            if wayfinder::sidebar::add_bookmark(&path) {
                w.imp().sidebar.refresh_bookmarks();
                w.announce(
                    &format!("Bookmarked {path}"),
                    AccessibleAnnouncementPriority::Medium,
                );
            } else {
                w.announce("Already bookmarked", AccessibleAnnouncementPriority::Medium);
            }
        });
        window.add_action(&action);

        // Undo (restore last trashed files)
        let w = window.clone();
        let action = gio::SimpleAction::new("undo", None);
        action.connect_activate(move |_, _| {
            w.undo_trash();
        });
        window.add_action(&action);

        // Open terminal here
        let w = window.clone();
        let action = gio::SimpleAction::new("terminal-here", None);
        action.connect_activate(move |_, _| {
            w.open_terminal_here();
        });
        window.add_action(&action);

        // Go to folder — same as Ctrl+L location dialog
        // (the "location-bar" action is registered in setup_location_dialog)

        // Batch rename
        let w = window.clone();
        let action = gio::SimpleAction::new("batch-rename", None);
        action.connect_activate(move |_, _| {
            w.batch_rename();
        });
        window.add_action(&action);

        // Zoom actions
        let w = window.clone();
        let action = gio::SimpleAction::new("zoom-in", None);
        action.connect_activate(move |_, _| {
            w.apply_zoom(10);
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("zoom-out", None);
        action.connect_activate(move |_, _| {
            w.apply_zoom(-10);
        });
        window.add_action(&action);

        let w = window.clone();
        let action = gio::SimpleAction::new("zoom-reset", None);
        action.connect_activate(move |_, _| {
            w.set_zoom(100);
        });
        window.add_action(&action);
    }

    fn connect_activation(&self) {
        let window = self.obj().clone();
        self.list_view
            .column_view()
            .connect_activate(move |_cv, pos| {
                let imp = window.imp();
                if let Some(item) = imp.selection.item(pos) {
                    let file = item.downcast_ref::<FileObject>().unwrap();
                    if file.is_directory() {
                        window.navigate_to_path(&file.path());
                    } else {
                        window.open_file(file);
                    }
                }
            });
    }

    fn connect_grid_activation(&self) {
        let window = self.obj().clone();
        self.grid_view
            .grid_view()
            .connect_activate(move |_gv, pos| {
                let imp = window.imp();
                if let Some(item) = imp.selection.item(pos) {
                    let file = item.downcast_ref::<FileObject>().unwrap();
                    if file.is_directory() {
                        window.navigate_to_path(&file.path());
                    } else {
                        window.open_file(file);
                    }
                }
            });
    }

    fn setup_location_dialog(&self) {
        // Ctrl+L action: open a "Go to location" dialog with Tab autocomplete
        let w = self.obj().clone();
        let action = gio::SimpleAction::new("location-bar", None);
        action.connect_activate(move |_, _| {
            let window = w.clone();

            let dlg = gtk::Window::builder()
                .title("Go to Location")
                .modal(true)
                .transient_for(&window)
                .default_width(500)
                .build();

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
            vbox.set_margin_top(12);
            vbox.set_margin_bottom(12);
            vbox.set_margin_start(12);
            vbox.set_margin_end(12);

            let label = gtk::Label::new(Some("Enter a path (Tab to autocomplete):"));
            vbox.append(&label);

            let entry = gtk::Entry::builder()
                .hexpand(true)
                .text(window.imp().model.current_path())
                .build();
            entry.update_property(&[gtk::accessible::Property::Label("Location path")]);
            entry.select_region(0, -1);
            vbox.append(&entry);

            let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            button_box.set_halign(gtk::Align::End);
            let cancel_btn = gtk::Button::with_label("Cancel");
            let go_btn = gtk::Button::with_label("Go");
            go_btn.add_css_class("suggested-action");
            button_box.append(&cancel_btn);
            button_box.append(&go_btn);
            vbox.append(&button_box);

            dlg.set_child(Some(&vbox));

            // Tab autocomplete + Escape to cancel
            let tab_ctrl = gtk::EventControllerKey::new();
            tab_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
            let entry_for_tab = entry.clone();
            let dlg_for_tab = dlg.clone();
            let dlg_for_esc = dlg.clone();
            // A relative path is taken from the folder being shown.
            let here = window.imp().model.current_path();
            let here_for_go = here.clone();
            tab_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
                use gtk::gdk;
                if key == gdk::Key::Escape {
                    dlg_for_esc.close();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Tab && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                    let text = entry_for_tab.text().to_string();
                    if text.is_empty() {
                        return glib::Propagation::Proceed;
                    }

                    // Completing lists a directory, which may be on a slow
                    // mount; apply the result only if the text is unchanged.
                    let entry = entry_for_tab.clone();
                    let dlg = dlg_for_tab.clone();
                    let dlg_slow = dlg_for_tab.clone();
                    let waiting = wayfinder::background::waiting_message(
                        &wayfinder::path_complete::expand_tilde(&text),
                    );
                    let typed = wayfinder::path_complete::absolute(&text, &here);
                    wayfinder::background::run(
                        move || wayfinder::path_complete::complete(&typed),
                        move |completed| {
                            if entry.text() != text.as_str() {
                                return;
                            }
                            use wayfinder::path_complete::Completion;
                            match completed {
                                Completion::Text(completed) => {
                                    let added =
                                        wayfinder::path_complete::added_text(&text, &completed);
                                    entry.set_text(&completed);
                                    entry.set_position(-1);
                                    if !added.is_empty() {
                                        dlg.announce(added, AccessibleAnnouncementPriority::Medium);
                                    }
                                }
                                Completion::Several(n) => dlg.announce(
                                    &format!("{n} matches"),
                                    AccessibleAnnouncementPriority::Medium,
                                ),
                                Completion::NoMatch if text.contains('/') => dlg.announce(
                                    "No completions",
                                    AccessibleAnnouncementPriority::Medium,
                                ),
                                Completion::NoMatch => {}
                            }
                        },
                        move || {
                            if let Some(msg) = waiting {
                                dlg_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                            }
                        },
                    );
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            entry.add_controller(tab_ctrl);

            // Cancel
            let d = dlg.clone();
            cancel_btn.connect_clicked(move |_| d.close());

            // Go — navigate and close
            let d = dlg.clone();
            let entry_clone = entry.clone();
            let w = window.clone();
            let do_go = move || {
                let path = wayfinder::path_complete::expand_tilde(
                    &wayfinder::path_complete::absolute(&entry_clone.text(), &here_for_go),
                );
                w.navigate_to_path(&path);
                d.close();
            };

            let do_go_clone = do_go.clone();
            go_btn.connect_clicked(move |_| do_go_clone());
            entry.connect_activate(move |_| do_go());

            dlg.present();
            entry.grab_focus();
        });
        self.obj().add_action(&action);
    }

    fn connect_list_key_navigation(&self) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let window = self.obj().clone();

        controller.connect_key_pressed(move |_controller, key, _code, mods| {
            use gtk::gdk;
            let imp = window.imp();

            if key == gdk::Key::Tab && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                if imp.back_button.is_sensitive() {
                    imp.back_button.grab_focus();
                } else if imp.forward_button.is_sensitive() {
                    imp.forward_button.grab_focus();
                } else if imp.up_button.is_sensitive() {
                    imp.up_button.grab_focus();
                } else if !imp.location_entry.grab_focus() {
                    // Nowhere of ours to go: let GTK move focus on.
                    return glib::Propagation::Proceed;
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::ISO_Left_Tab
                || (key == gdk::Key::Tab && mods.contains(gdk::ModifierType::SHIFT_MASK))
            {
                // The location entry isn't always in the window: if it can't
                // take focus, let GTK move focus back, rather than trap it.
                if imp.location_entry.grab_focus() {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            } else if key == gdk::Key::Left && !mods.contains(gdk::ModifierType::ALT_MASK) {
                let col = imp.current_column.get();
                if col > 0 {
                    imp.current_column.set(col - 1);
                    imp.announce_current_cell(&window);
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::Right && !mods.contains(gdk::ModifierType::ALT_MASK) {
                let col = imp.current_column.get();
                if col + 1 < COLUMN_NAMES.len() {
                    imp.current_column.set(col + 1);
                    imp.announce_current_cell(&window);
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::space && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                // Space: toggle selection of current item
                if let Some(item) = imp.selection.selected_item() {
                    if let Some(file) = item.downcast_ref::<FileObject>() {
                        let mut sel = imp.file_selection.borrow_mut();
                        let selected = sel.toggle(&file.path());
                        let count = sel.count();
                        drop(sel);
                        if selected {
                            window.announce(
                                &format!("{} selected, {} total", file.name(), count),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        } else {
                            window.announce(
                                &format!("{} deselected, {} total", file.name(), count),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                        window.update_status();
                    }
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::space && mods.contains(gdk::ModifierType::SHIFT_MASK) {
                // Shift+Space: start or finish range selection
                let mut sel = imp.file_selection.borrow_mut();
                let pos = imp.selection.selected();
                if let Some(anchor) = sel.range_anchor {
                    // Finish range — select all items between anchor and current
                    // No row selected (an empty or filtered-out list) gives
                    // an invalid position: never loop to it.
                    let n_items = imp.selection.n_items();
                    let start = anchor.min(pos);
                    let end = anchor.max(pos).min(n_items.saturating_sub(1));
                    for i in (start..=end).take_while(|_| n_items > 0) {
                        if let Some(item) = imp.selection.model().and_then(|m| m.item(i)) {
                            if let Some(file) = item.downcast_ref::<FileObject>() {
                                sel.selected.insert(file.path());
                            }
                        }
                    }
                    let count = sel.count();
                    sel.range_anchor = None;
                    drop(sel);
                    window.announce(
                        &format!("Selection finished: {count} files selected"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    window.update_status();
                } else {
                    // Start range
                    sel.range_anchor = Some(pos);
                    drop(sel);
                    window.announce("Selection started", AccessibleAnnouncementPriority::Medium);
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::a && mods.contains(gdk::ModifierType::CONTROL_MASK) {
                // Ctrl+A: select all files
                let mut sel = imp.file_selection.borrow_mut();
                let model = &imp.model.filter_model;
                for i in 0..model.n_items() {
                    if let Some(item) = model.item(i) {
                        if let Some(file) = item.downcast_ref::<FileObject>() {
                            sel.selected.insert(file.path());
                        }
                    }
                }
                let count = sel.count();
                drop(sel);
                window.announce(
                    &format!("{count} files selected"),
                    AccessibleAnnouncementPriority::Medium,
                );
                window.update_status();
                glib::Propagation::Stop
            } else if key == gdk::Key::Escape {
                // Escape: cancel range selection or clear selection
                let mut sel = imp.file_selection.borrow_mut();
                if sel.range_anchor.is_some() {
                    sel.range_anchor = None;
                    drop(sel);
                    window.announce(
                        "Selection cancelled",
                        AccessibleAnnouncementPriority::Medium,
                    );
                } else if sel.count() > 0 {
                    sel.clear();
                    drop(sel);
                    window.announce("Selection cleared", AccessibleAnnouncementPriority::Medium);
                    window.update_status();
                } else {
                    drop(sel);
                    return glib::Propagation::Proceed;
                }
                glib::Propagation::Stop
            } else if !mods.contains(gdk::ModifierType::CONTROL_MASK)
                && !mods.contains(gdk::ModifierType::ALT_MASK)
                && key != gdk::Key::Return
                && key != gdk::Key::KP_Enter
                && key != gdk::Key::Delete
                && key != gdk::Key::BackSpace
            {
                // Type-ahead: navigate to file starting with typed characters
                if let Some(ch) = key.to_unicode() {
                    if ch.is_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                        // Bump generation to invalidate any pending timer
                        let gen = imp.type_ahead_generation.get().wrapping_add(1);
                        imp.type_ahead_generation.set(gen);

                        // Append character to buffer
                        imp.type_ahead_buffer.borrow_mut().push(ch);
                        let query = imp.type_ahead_buffer.borrow().to_lowercase();

                        // Find first matching item
                        if let Some(model) = imp.selection.model() {
                            for i in 0..model.n_items() {
                                if let Some(item) = model.item(i) {
                                    if let Some(file) = item.downcast_ref::<FileObject>() {
                                        if file.name().to_lowercase().starts_with(&query) {
                                            imp.selection.set_selected(i);
                                            window.restore_focus_to_selected();
                                            window.announce(
                                                &file.a11y_name(),
                                                AccessibleAnnouncementPriority::Medium,
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // Reset buffer after 800ms of no typing
                        let buffer = imp.type_ahead_buffer.clone();
                        let gen_cell = imp.type_ahead_generation.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(800),
                            move || {
                                if gen_cell.get() == gen {
                                    buffer.borrow_mut().clear();
                                }
                            },
                        );

                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            } else {
                glib::Propagation::Proceed
            }
        });

        self.list_view.column_view().add_controller(controller);

        // Announce column data on selection change in list view only.
        // Icon view is a native GtkListView — Orca reads it directly.
        // (Weak: the window owns the selection.)
        let window2 = self.obj().downgrade();
        self.selection.connect_selected_item_notify(move |_sel| {
            let Some(window2) = window2.upgrade() else {
                return;
            };
            if window2.imp().current_view.get() == ViewMode::List {
                window2.imp().announce_current_cell(&window2);
            }
        });
    }

    fn connect_grid_key_navigation(&self) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let window = self.obj().clone();

        controller.connect_key_pressed(move |_controller, key, _code, mods| {
            use gtk::gdk;
            let imp = window.imp();

            if key == gdk::Key::Tab && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                if imp.back_button.is_sensitive() {
                    imp.back_button.grab_focus();
                } else if imp.forward_button.is_sensitive() {
                    imp.forward_button.grab_focus();
                } else if imp.up_button.is_sensitive() {
                    imp.up_button.grab_focus();
                } else if !imp.location_entry.grab_focus() {
                    // Nowhere of ours to go: let GTK move focus on.
                    return glib::Propagation::Proceed;
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::ISO_Left_Tab
                || (key == gdk::Key::Tab && mods.contains(gdk::ModifierType::SHIFT_MASK))
            {
                // The location entry isn't always in the window: if it can't
                // take focus, let GTK move focus back, rather than trap it.
                if imp.location_entry.grab_focus() {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            } else if key == gdk::Key::space && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                if let Some(item) = imp.selection.selected_item() {
                    if let Some(file) = item.downcast_ref::<FileObject>() {
                        let mut sel = imp.file_selection.borrow_mut();
                        let selected = sel.toggle(&file.path());
                        let count = sel.count();
                        drop(sel);
                        if selected {
                            window.announce(
                                &format!("{} selected, {} total", file.name(), count),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        } else {
                            window.announce(
                                &format!("{} deselected, {} total", file.name(), count),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                        window.update_status();
                    }
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::space && mods.contains(gdk::ModifierType::SHIFT_MASK) {
                let mut sel = imp.file_selection.borrow_mut();
                let pos = imp.selection.selected();
                if let Some(anchor) = sel.range_anchor {
                    // No row selected (an empty or filtered-out list) gives
                    // an invalid position: never loop to it.
                    let n_items = imp.selection.n_items();
                    let start = anchor.min(pos);
                    let end = anchor.max(pos).min(n_items.saturating_sub(1));
                    for i in (start..=end).take_while(|_| n_items > 0) {
                        if let Some(item) = imp.selection.model().and_then(|m| m.item(i)) {
                            if let Some(file) = item.downcast_ref::<FileObject>() {
                                sel.selected.insert(file.path());
                            }
                        }
                    }
                    let count = sel.count();
                    sel.range_anchor = None;
                    drop(sel);
                    window.announce(
                        &format!("Selection finished: {count} files selected"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    window.update_status();
                } else {
                    sel.range_anchor = Some(pos);
                    drop(sel);
                    window.announce("Selection started", AccessibleAnnouncementPriority::Medium);
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::a && mods.contains(gdk::ModifierType::CONTROL_MASK) {
                // Ctrl+A: select all files
                let mut sel = imp.file_selection.borrow_mut();
                let model = &imp.model.filter_model;
                for i in 0..model.n_items() {
                    if let Some(item) = model.item(i) {
                        if let Some(file) = item.downcast_ref::<FileObject>() {
                            sel.selected.insert(file.path());
                        }
                    }
                }
                let count = sel.count();
                drop(sel);
                window.announce(
                    &format!("{count} files selected"),
                    AccessibleAnnouncementPriority::Medium,
                );
                window.update_status();
                glib::Propagation::Stop
            } else if key == gdk::Key::Escape {
                let mut sel = imp.file_selection.borrow_mut();
                if sel.range_anchor.is_some() {
                    sel.range_anchor = None;
                    drop(sel);
                    window.announce(
                        "Selection cancelled",
                        AccessibleAnnouncementPriority::Medium,
                    );
                } else if sel.count() > 0 {
                    sel.clear();
                    drop(sel);
                    window.announce("Selection cleared", AccessibleAnnouncementPriority::Medium);
                    window.update_status();
                } else {
                    drop(sel);
                    return glib::Propagation::Proceed;
                }
                glib::Propagation::Stop
            } else if !mods.contains(gdk::ModifierType::CONTROL_MASK)
                && !mods.contains(gdk::ModifierType::ALT_MASK)
                && key != gdk::Key::Return
                && key != gdk::Key::KP_Enter
                && key != gdk::Key::Delete
                && key != gdk::Key::BackSpace
            {
                // Type-ahead: navigate to file starting with typed characters
                if let Some(ch) = key.to_unicode() {
                    if ch.is_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                        // Bump generation to invalidate any pending timer
                        let gen = imp.type_ahead_generation.get().wrapping_add(1);
                        imp.type_ahead_generation.set(gen);

                        // Append character to buffer
                        imp.type_ahead_buffer.borrow_mut().push(ch);
                        let query = imp.type_ahead_buffer.borrow().to_lowercase();

                        // Find first matching item
                        if let Some(model) = imp.selection.model() {
                            for i in 0..model.n_items() {
                                if let Some(item) = model.item(i) {
                                    if let Some(file) = item.downcast_ref::<FileObject>() {
                                        if file.name().to_lowercase().starts_with(&query) {
                                            imp.selection.set_selected(i);
                                            window.restore_focus_to_selected();
                                            window.announce(
                                                &file.a11y_name(),
                                                AccessibleAnnouncementPriority::Medium,
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // Reset buffer after 800ms of no typing
                        let buffer = imp.type_ahead_buffer.clone();
                        let gen_cell = imp.type_ahead_generation.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(800),
                            move || {
                                if gen_cell.get() == gen {
                                    buffer.borrow_mut().clear();
                                }
                            },
                        );

                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            } else {
                glib::Propagation::Proceed
            }
        });

        self.grid_view.grid_view().add_controller(controller);
    }

    fn connect_search(&self) {
        let window = self.obj().clone();
        let search_bar = self.search_bar.clone();

        self.search_entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            let imp = window.imp();
            // Cleared already (on leaving a folder): nothing to announce —
            // the count would be the old folder's.
            let was_active = imp.model.search.is_active();
            imp.model.search.set_query(&text);
            window.update_status();

            if text.is_empty() && !was_active {
                return;
            }
            if text.is_empty() {
                let count = imp.model.item_count();
                window.announce(
                    &format!("Filter cleared, {count} items"),
                    AccessibleAnnouncementPriority::Medium,
                );
            } else {
                let count = imp.model.item_count();
                window.announce(
                    &format!("{count} items match '{text}'"),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
        });

        // When search bar closes, clear the filter
        let window2 = self.obj().clone();
        search_bar.connect_search_mode_enabled_notify(move |bar| {
            if !bar.is_search_mode() {
                window2.imp().model.search.clear();
                window2.imp().search_entry.set_text("");
                window2.update_status();
                window2.focus_current_view();
            }
        });
    }

    fn connect_sidebar(&self) {
        let window = self.obj().clone();

        self.sidebar.connect_place_activated(move |path| {
            if path.starts_with("trash:") || path.starts_with("recent:") {
                window.load_special_uri(&path);
            } else {
                window.navigate_to_path(&path);
            }
        });
        // Right-click on Bin shows "Empty Bin" menu
        let window2 = self.obj().clone();
        self.sidebar.connect_trash_right_click(move || {
            let menu = gio::Menu::new();
            menu.append(Some("Open Bin"), Some("win.open-trash"));
            menu.append(Some("Empty Bin"), Some("win.empty-trash"));

            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            popover.set_parent(window2.upcast_ref::<gtk::Widget>());
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(10, 10, 1, 1)));
            popover.set_has_arrow(false);

            let w = window2.clone();
            popover.connect_closed(move |pop| {
                pop.unparent();
                let win = w.clone();
                glib::idle_add_local_once(move || {
                    win.restore_focus_to_selected();
                });
            });

            popover.popup();
        });

        // Right-click on sidebar (non-bookmark, non-trash) shows "Edit Sidebar" menu
        self.sidebar
            .connect_sidebar_edit_menu(&self.obj().clone().upcast());

        // Register open-trash action
        let w = self.obj().clone();
        let action = gio::SimpleAction::new("open-trash", None);
        action.connect_activate(move |_, _| {
            w.load_special_uri("trash:///");
        });
        self.obj().add_action(&action);
    }

    fn connect_context_menu(&self) {
        // Right-click on list view
        let click_controller = gtk::GestureClick::new();
        click_controller.set_button(3); // right button
        let window = self.obj().clone();
        click_controller.connect_pressed(move |gesture, _n, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            // On a file: its menu. On empty space: the folder's own menu
            // (Paste, New Folder), not one for whichever file has focus.
            if wayfinder::views::take_right_clicked_item() {
                window.drop_marks_unless_selected_is_marked();
                crate::context_menu::show_context_menu(&window, x, y);
            } else {
                crate::context_menu::show_background_menu(&window, x, y, window.is_in_trash());
            }
        });
        self.list_view
            .column_view()
            .add_controller(click_controller);

        // Right-click on grid view
        let click_controller2 = gtk::GestureClick::new();
        click_controller2.set_button(3);
        let window2 = self.obj().clone();
        click_controller2.connect_pressed(move |gesture, _n, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            // On a file: its menu. On empty space: the folder's own menu
            // (Paste, New Folder), not one for whichever file has focus.
            if wayfinder::views::take_right_clicked_item() {
                window2.drop_marks_unless_selected_is_marked();
                crate::context_menu::show_context_menu(&window2, x, y);
            } else {
                crate::context_menu::show_background_menu(&window2, x, y, window2.is_in_trash());
            }
        });
        self.grid_view.grid_view().add_controller(click_controller2);

        // Shift+F10 / Menu key — direct key controller on both views (capture phase)
        // because the shortcut controller doesn't reliably catch Shift+F10
        let w = self.obj().clone();
        let list_key = gtk::EventControllerKey::new();
        list_key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w1 = w.clone();
        list_key.connect_key_pressed(move |_ctrl, key, _code, mods| {
            use gtk::gdk;
            if key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK)
                || key == gdk::Key::Menu
            {
                crate::context_menu::show_context_menu(&w1, 100.0, 100.0);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.list_view.column_view().add_controller(list_key);

        let grid_key = gtk::EventControllerKey::new();
        grid_key.set_propagation_phase(gtk::PropagationPhase::Capture);
        grid_key.connect_key_pressed(move |_ctrl, key, _code, mods| {
            use gtk::gdk;
            if key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK)
                || key == gdk::Key::Menu
            {
                crate::context_menu::show_context_menu(&w, 100.0, 100.0);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.grid_view.grid_view().add_controller(grid_key);

        // Also keep the action for the shortcut controller (works from other widgets)
        let w3 = self.obj().clone();
        let action = gio::SimpleAction::new("context-menu", None);
        action.connect_activate(move |_, _| {
            crate::context_menu::show_context_menu(&w3, 100.0, 100.0);
        });
        self.obj().add_action(&action);
    }

    fn connect_drop_targets(&self) {
        let window = self.obj().clone();

        // Drop target for list view
        let drop_target = gtk::DropTarget::new(
            glib::Type::STRING,
            gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
        );
        let w = window.clone();
        drop_target.connect_drop(move |_target, value, _x, _y| {
            if let Ok(uri_str) = value.get::<String>() {
                w.handle_drop(&uri_str);
                return true;
            }
            false
        });
        self.list_view.column_view().add_controller(drop_target);

        // Drop target for grid view
        let drop_target = gtk::DropTarget::new(
            glib::Type::STRING,
            gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
        );
        drop_target.connect_drop(move |_target, value, _x, _y| {
            if let Ok(uri_str) = value.get::<String>() {
                window.handle_drop(&uri_str);
                return true;
            }
            false
        });
        self.grid_view.grid_view().add_controller(drop_target);
    }

    fn announce_current_cell(&self, window: &super::WayfinderWindow) {
        let col = self.current_column.get();
        let col_name = COLUMN_NAMES[col];

        if let Some(item) = self.selection.selected_item() {
            let file = item.downcast_ref::<FileObject>().unwrap();
            let value = match col {
                0 => file.name(),
                1 => file.size_display(),
                2 => file.modified_display(),
                3 => file.file_type_name(),
                _ => String::new(),
            };
            let announcement = format!("{col_name}: {value}");
            window.announce(&announcement, AccessibleAnnouncementPriority::Medium);
        }
    }

    fn connect_nav_buttons(&self) {
        let window = self.obj().clone();
        self.back_button.connect_clicked(move |_| {
            let path = window.imp().nav.borrow_mut().go_back().cloned();
            if let Some(path) = path {
                window.load_directory(&path.to_string_lossy());
            }
        });

        let window = self.obj().clone();
        self.forward_button.connect_clicked(move |_| {
            let path = window.imp().nav.borrow_mut().go_forward().cloned();
            if let Some(path) = path {
                window.load_directory(&path.to_string_lossy());
            }
        });

        let window = self.obj().clone();
        self.up_button.connect_clicked(move |_| {
            let parent = window.imp().nav.borrow().go_up();
            if let Some(parent) = parent {
                window.navigate_to_path(&parent.to_string_lossy());
            }
        });
    }
}
