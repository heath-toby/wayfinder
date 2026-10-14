mod imp;

use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

glib::wrapper! {
    pub struct WayfinderSidebar(ObjectSubclass<imp::SidebarInner>);
}

fn bookmarks_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gtk-3.0")
        .join("bookmarks")
}

/// The bookmarks, to show; none if they can't be read.
pub fn load_bookmarks() -> Vec<(String, String)> {
    read_bookmarks().unwrap_or_default()
}

/// The bookmarks, to change: an error if the file is there but can't be
/// read (or isn't UTF-8) — writing it back would lose them all, other
/// apps' too.
fn read_bookmarks() -> Result<Vec<(String, String)>, String> {
    read_bookmarks_from(&bookmarks_file())
}

fn read_bookmarks_from(file: &std::path::Path) -> Result<Vec<(String, String)>, String> {
    let contents = match std::fs::read_to_string(file) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Couldn't read the bookmarks: {e}")),
    };
    Ok(parse_bookmarks(&contents))
}

fn parse_bookmarks(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (uri, display) = if let Some((u, d)) = line.split_once(' ') {
                (u.to_string(), d.to_string())
            } else {
                (line.to_string(), default_bookmark_name(line))
            };
            Some((uri, display))
        })
        .collect()
}

/// The name a bookmark with none given is shown by: the folder's own
/// (decoded, so `My%20Docs` is read as "My Docs").
fn default_bookmark_name(uri: &str) -> String {
    bookmark_path(uri)
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| crate::file_model::display_name(uri))
}

/// The bookmarks file's text for `bookmarks`. One shown by its folder's own
/// name is written without one, as it was read: no label is added to the
/// user's bookmarks (other apps' too) just by reordering them.
fn bookmarks_text(bookmarks: &[(String, String)]) -> String {
    bookmarks
        .iter()
        .map(|(uri, name)| {
            if *name == default_bookmark_name(uri) {
                uri.clone()
            } else {
                format!("{uri} {name}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn save_bookmarks(bookmarks: &[(String, String)]) -> Result<(), String> {
    let contents = bookmarks_text(bookmarks);
    // (Through a link, to a dotfiles folder say, to the file it leads to.)
    crate::state::write_atomically(&bookmarks_file(), contents.as_bytes())
        .map_err(|e| format!("Couldn't save the bookmarks: {e}"))
}

thread_local! {
    /// Every sidebar (one per window): a bookmark change shows in all.
    static SIDEBARS: std::cell::RefCell<Vec<glib::WeakRef<WayfinderSidebar>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Show the bookmarks as they are now in every sidebar but `except`.
pub fn refresh_all_bookmarks(except: Option<&WayfinderSidebar>) {
    for sidebar in live_sidebars() {
        if Some(&sidebar) != except {
            sidebar.refresh_bookmarks();
        }
    }
}

fn live_sidebars() -> Vec<WayfinderSidebar> {
    SIDEBARS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|w| w.upgrade().is_some());
        s.iter().filter_map(|w| w.upgrade()).collect()
    })
}

/// The local folder a bookmark URI names (percent-decoded); `None` for
/// other URIs (`sftp://…`), which Wayfinder can't open from the sidebar.
pub fn bookmark_path(uri: &str) -> Option<String> {
    uri.starts_with("file://")
        .then(|| gtk::gio::File::for_uri(uri).path())
        .flatten()
        .map(|p| p.to_string_lossy().to_string())
}

/// Where the bookmark for `path` is in `bookmarks`.
fn bookmark_index(bookmarks: &[(String, String)], path: &str) -> Option<usize> {
    bookmarks
        .iter()
        .position(|(u, _)| bookmark_path(u).as_deref() == Some(path))
}

/// Bookmark `path`; false if it already is.
pub fn add_bookmark(path: &str) -> Result<bool, String> {
    // Encoded as GTK writes it: a space would end the URI on reading.
    let uri = gtk::gio::File::for_path(path).uri().to_string();
    let mut bookmarks = read_bookmarks()?;
    if bookmark_index(&bookmarks, path).is_some() {
        return Ok(false); // already exists
    }
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    bookmarks.push((uri, name));
    save_bookmarks(&bookmarks)?;
    Ok(true)
}

pub fn remove_bookmark(path: &str) -> Result<(), String> {
    let mut bookmarks = read_bookmarks()?;
    bookmarks.retain(|(u, _)| bookmark_path(u).as_deref() != Some(path));
    save_bookmarks(&bookmarks)
}

/// Remove a bookmark's row (and the bookmark), say so, and put focus on a
/// neighbouring row that can take it.
fn remove_bookmark_row(list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let name = row.widget_name().to_string();
    let Some(path) = name.strip_prefix("bookmark:") else {
        return;
    };
    // Said by the name its row shows (and Orca read), not its folder's.
    let display = row
        .child()
        .and_then(|b| b.last_child())
        .and_downcast::<gtk::Label>()
        .map(|l| l.text().to_string())
        .unwrap_or_else(|| {
            std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string())
        });
    if let Err(e) = remove_bookmark(path) {
        list.announce(&e, gtk::AccessibleAnnouncementPriority::High);
        return;
    }
    let idx = row.index();
    // Every sidebar, this one too, shows the bookmarks as they are now
    // (another entry for the same folder goes as well, and the separator
    // with the last one).
    refresh_all_bookmarks(None);
    // Focus where it was: the row now there, or the nearest one before it
    // that can take focus (not a separator).
    let next = list
        .row_at_index(idx)
        .filter(|r| r.is_selectable())
        .or_else(|| {
            (0..idx)
                .rev()
                .filter_map(|i| list.row_at_index(i))
                .find(|r| r.is_selectable())
        });
    if let Some(next) = next {
        list.select_row(Some(&next));
        next.grab_focus();
    }
    list.announce(
        &format!("Bookmark {display} removed"),
        gtk::AccessibleAnnouncementPriority::Medium,
    );
}

thread_local! {
    /// Places activated so far (to tell whether the user has moved on).
    static ACTIVATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Volumes being mounted (by UUID).
    static MOUNTING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The widget names of `list`'s rows, in order.
fn row_names(list: &gtk::ListBox) -> Vec<String> {
    let mut names = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        names.push(widget.widget_name().to_string());
        child = widget.next_sibling();
    }
    names
}

/// Where bookmarks go: after the places at the top, whichever of them are
/// shown and in whatever order (not after File System, which may be hidden).
fn bookmarks_insert_pos(names: &[String]) -> usize {
    names
        .iter()
        .take_while(|n| {
            n.starts_with("place:") && *n != "place:recent:///" && *n != "place:trash:///"
        })
        .count()
}

/// Where drives go: before the separator above Recent, or at the end while
/// the list is being built.
fn volumes_insert_pos(names: &[String]) -> usize {
    match names.iter().position(|n| n == "place:recent:///") {
        Some(pos) if pos > 0 => pos - 1,
        _ => names.len(),
    }
}

/// Eject (or unmount) the drive mounted at `mount_path`, and say how it
/// went. `widget` is in the window that asked: GTK asks it for any password.
fn eject_mount_at(widget: &gtk::Widget, mount_path: &str, label: &str) {
    let mount = gio::VolumeMonitor::get().mounts().into_iter().find(|m| {
        m.default_location()
            .path()
            .is_some_and(|p| p.to_string_lossy() == mount_path)
    });
    let Some(mount) = mount else {
        widget.announce(
            &format!("{label} is no longer mounted"),
            gtk::AccessibleAnnouncementPriority::Medium,
        );
        return;
    };
    let window = widget.root().and_downcast::<gtk::Window>();
    let op = gtk::MountOperation::new(window.as_ref());
    // Said by the window: the drive's row goes once it's ejected.
    let w: gtk::Widget = window.map(|w| w.upcast()).unwrap_or_else(|| widget.clone());
    let name = label.to_string();
    let done = move |result: Result<(), glib::Error>| match result {
        Ok(()) => w.announce(
            &format!("{name} can be removed"),
            gtk::AccessibleAnnouncementPriority::Medium,
        ),
        Err(e) => {
            log::error!("Failed to eject {name}: {e}");
            w.announce(
                &format!("Couldn't eject {name}: {e}"),
                gtk::AccessibleAnnouncementPriority::High,
            );
        }
    };
    if mount.can_eject() {
        mount.eject_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&op),
            gio::Cancellable::NONE,
            done,
        );
    } else if mount.can_unmount() {
        mount.unmount_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&op),
            gio::Cancellable::NONE,
            done,
        );
    } else {
        widget.announce(
            &format!("{label} can't be ejected"),
            gtk::AccessibleAnnouncementPriority::Medium,
        );
    }
}

/// Focus the row named `name`, or, if it's gone, the nearest selectable
/// row at or before `index`.
fn refocus_row(list: &gtk::ListBox, name: &str, index: i32) {
    let mut row = None;
    let mut i = 0;
    while let Some(r) = list.row_at_index(i) {
        if r.widget_name() == name {
            row = Some(r);
            break;
        }
        i += 1;
    }
    let row = row.or_else(|| {
        (0..=index)
            .rev()
            .filter_map(|i| list.row_at_index(i))
            .find(|r| r.is_selectable())
    });
    if let Some(row) = row {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

/// An "Eject" item for `row`'s menu, if it's a drive that can be ejected
/// (otherwise the eject button beside it is out of the keyboard's reach).
fn eject_item(row: &gtk::ListBoxRow, popover: &gtk::Popover) -> Option<gtk::Button> {
    let name = row.widget_name();
    let mount_path = name.strip_prefix("volume:")?.to_string();
    let mount = gio::VolumeMonitor::get().mounts().into_iter().find(|m| {
        m.default_location()
            .path()
            .is_some_and(|p| p.to_string_lossy() == mount_path)
    })?;
    if !(mount.can_eject() || mount.can_unmount()) {
        return None;
    }
    let label = mount.name().to_string();
    let btn = gtk::Button::with_label(&format!("Eject {label}"));
    btn.add_css_class("flat");
    let pop = popover.clone();
    let row = row.clone();
    btn.connect_clicked(move |_| {
        pop.popdown();
        eject_mount_at(row.upcast_ref(), &mount_path, &label);
    });
    Some(btn)
}

/// `item` and, below it, Wayfinder's settings: the sidebar's menu is
/// where they live.
fn with_settings(item: &gtk::Widget) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add_css_class("menu");
    vbox.append(item);
    vbox.append(&settings_check());
    vbox
}

/// "Reopen All Windows at Startup", on or off. A check box, so Orca says
/// which; changing it is saved at once.
pub fn settings_check() -> gtk::CheckButton {
    let check = gtk::CheckButton::with_label("Reopen All Windows at Startup");
    check.set_active(crate::state::load_reopen_windows());
    // (Orca reads the box ticked or not; nothing more to say.)
    check.connect_toggled(|check| crate::state::save_reopen_windows(check.is_active()));
    check
}

/// Focus the first item of `popover`'s menu once it's on screen (not when
/// it's "shown": it isn't yet, and focus would stay behind it).
/// (Also when the keyboard reaches it, which on Wayland comes a moment
/// later, and once more shortly after — each only if nothing in it has
/// focus yet.)
fn focus_first_when_shown(popover: &gtk::Popover) {
    let focus_first = {
        let pop = popover.downgrade();
        move || {
            let Some(pop) = pop.upgrade().filter(|p| p.is_visible()) else {
                return;
            };
            let inside = pop
                .root()
                .and_then(|r| r.focus())
                .is_some_and(|f| f != pop.clone().upcast::<gtk::Widget>() && f.is_ancestor(&pop));
            if !inside {
                if let Some(first) = pop.child().and_then(|menu| menu.first_child()) {
                    first.grab_focus();
                }
            }
        }
    };
    let keyboard = gtk::EventControllerFocus::new();
    let f = focus_first.clone();
    keyboard.connect_enter(move |_| {
        let f = f.clone();
        glib::idle_add_local_once(f);
    });
    popover.add_controller(keyboard);
    popover.connect_map(move |_| {
        glib::idle_add_local_once(focus_first.clone());
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), focus_first.clone());
    });
}

/// The menu for a bookmark row: Remove Bookmark, Edit Sidebar.
fn show_bookmark_menu(
    sidebar: &WayfinderSidebar,
    parent: &gtk::Window,
    list: &gtk::ListBox,
    row: &gtk::ListBoxRow,
) {
    let popover = gtk::Popover::new();
    popover.set_parent(row);
    popover.set_has_arrow(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add_css_class("menu");

    let remove_btn = gtk::Button::with_label("Remove Bookmark");
    remove_btn.add_css_class("flat");
    let pop = popover.clone();
    let list_ref = list.clone();
    let row_ref = row.clone();
    remove_btn.connect_clicked(move |_| {
        pop.popdown();
        remove_bookmark_row(&list_ref, &row_ref);
    });
    vbox.append(&remove_btn);

    let edit_btn = gtk::Button::with_label("Edit Sidebar");
    edit_btn.add_css_class("flat");
    let s = sidebar.clone();
    let p = parent.clone();
    let pop2 = popover.clone();
    edit_btn.connect_clicked(move |_| {
        pop2.popdown();
        s.show_edit_dialog(&p);
    });
    vbox.append(&edit_btn);
    vbox.append(&settings_check());

    popover.set_child(Some(&vbox));
    popover.connect_closed(|pop| pop.unparent());
    focus_first_when_shown(&popover);
    popover.popup();
}

/// The menu for the Bin's row: Open Bin, Empty Bin, Edit Sidebar — on the
/// row, as the other rows' menus are.
fn show_trash_menu(sidebar: &WayfinderSidebar, parent: &gtk::Window, row: &gtk::ListBoxRow) {
    let popover = gtk::Popover::new();
    popover.set_parent(row);
    popover.set_has_arrow(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add_css_class("menu");
    for (label, action) in [
        ("Open Bin", "win.open-trash"),
        ("Empty Bin…", "win.empty-trash"),
    ] {
        let btn = gtk::Button::with_label(label);
        btn.add_css_class("flat");
        let pop = popover.clone();
        let p = parent.clone();
        btn.connect_clicked(move |_| {
            pop.popdown();
            let _ = p.activate_action(action, None);
        });
        vbox.append(&btn);
    }

    let edit_btn = gtk::Button::with_label("Edit Sidebar");
    edit_btn.add_css_class("flat");
    let s = sidebar.clone();
    let p = parent.clone();
    let pop = popover.clone();
    edit_btn.connect_clicked(move |_| {
        pop.popdown();
        s.show_edit_dialog(&p);
    });
    vbox.append(&edit_btn);
    vbox.append(&settings_check());

    popover.set_child(Some(&vbox));
    popover.connect_closed(|pop| pop.unparent());
    focus_first_when_shown(&popover);
    popover.popup();
}

struct PlaceEntry {
    name: &'static str,
    icon: &'static str,
    dir_fn: fn() -> Option<std::path::PathBuf>,
}

const PLACES: &[PlaceEntry] = &[
    PlaceEntry {
        name: "Home",
        icon: "user-home-symbolic",
        dir_fn: dirs::home_dir,
    },
    PlaceEntry {
        name: "Desktop",
        icon: "user-desktop-symbolic",
        dir_fn: dirs::desktop_dir,
    },
    PlaceEntry {
        name: "Documents",
        icon: "folder-documents-symbolic",
        dir_fn: dirs::document_dir,
    },
    PlaceEntry {
        name: "Downloads",
        icon: "folder-download-symbolic",
        dir_fn: dirs::download_dir,
    },
    PlaceEntry {
        name: "Music",
        icon: "folder-music-symbolic",
        dir_fn: dirs::audio_dir,
    },
    PlaceEntry {
        name: "Pictures",
        icon: "folder-pictures-symbolic",
        dir_fn: dirs::picture_dir,
    },
    PlaceEntry {
        name: "Videos",
        icon: "folder-videos-symbolic",
        dir_fn: dirs::video_dir,
    },
];

impl WayfinderSidebar {
    pub fn new() -> Self {
        let sidebar: Self = glib::Object::builder().build();
        SIDEBARS.with(|s| s.borrow_mut().push(sidebar.downgrade()));
        sidebar.populate_places();
        sidebar.connect_volume_signals();
        sidebar.connect_bookmark_delete_key();
        sidebar
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.imp().container
    }

    pub fn connect_place_activated<F: Fn(String) + 'static>(&self, callback: F) {
        let callback = std::rc::Rc::new(callback);
        self.imp()
            .places_list
            .connect_row_activated(move |list, row| {
                ACTIVATIONS.with(|n| n.set(n.get() + 1));
                let name = row.widget_name();
                if let Some(path) = name.strip_prefix("place:") {
                    callback(path.to_string());
                } else if let Some(path) = name.strip_prefix("bookmark:") {
                    callback(path.to_string());
                } else if let Some(path) = name.strip_prefix("volume:") {
                    callback(path.to_string());
                } else if name.starts_with("volume-unmounted:") {
                    // Unmounted volume — try to mount it
                    if let Some(uuid) = name.strip_prefix("volume-unmounted:") {
                        let vm = gio::VolumeMonitor::get();
                        for vol in vm.volumes() {
                            let vol_id = vol.uuid().map(|u| u.to_string()).unwrap_or_default();
                            if vol_id == uuid {
                                let label = vol.name().to_string();
                                // A second Enter while it's mounting: wait.
                                if MOUNTING.with(|m| m.borrow().contains(&vol_id)) {
                                    list.announce(
                                        &format!("Still mounting {label}"),
                                        gtk::AccessibleAnnouncementPriority::Medium,
                                    );
                                    break;
                                }
                                MOUNTING.with(|m| m.borrow_mut().push(vol_id.clone()));
                                let activation = ACTIVATIONS.with(|n| n.get());
                                // GTK's asks for a password, if needed, in a
                                // dialog over this window.
                                let window = list.root().and_downcast::<gtk::Window>();
                                let mount_op = gtk::MountOperation::new(window.as_ref());
                                let mounted = vol.clone();
                                let callback = callback.clone();
                                let list = list.clone();
                                list.announce(
                                    &format!("Mounting {label}"),
                                    gtk::AccessibleAnnouncementPriority::Medium,
                                );
                                // Once mounted, open it — as activating a
                                // mounted one does; if it fails, say why.
                                vol.mount(
                                    gio::MountMountFlags::NONE,
                                    Some(&mount_op),
                                    gio::Cancellable::NONE,
                                    move |result| {
                                        MOUNTING.with(|m| m.borrow_mut().retain(|v| *v != vol_id));
                                        match result {
                                            Ok(()) => {
                                                // Open it — unless the user has
                                                // chosen something else meanwhile.
                                                let moved_on =
                                                    ACTIVATIONS.with(|n| n.get()) != activation;
                                                let path = mounted
                                                    .get_mount()
                                                    .and_then(|m| m.default_location().path());
                                                match path {
                                                    Some(path) if !moved_on => {
                                                        callback(path.to_string_lossy().to_string())
                                                    }
                                                    _ => list.announce(
                                                        &format!("{label} mounted"),
                                                        gtk::AccessibleAnnouncementPriority::Medium,
                                                    ),
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("Failed to mount volume: {e}");
                                                list.announce(
                                                    &format!("Couldn't mount {label}: {e}"),
                                                    gtk::AccessibleAnnouncementPriority::High,
                                                );
                                            }
                                        }
                                    },
                                );
                                break;
                            }
                        }
                    }
                }
            });
    }

    /// Delete key removes the focused bookmark. Ctrl+Up/Down reorders bookmarks.
    fn connect_bookmark_delete_key(&self) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let list = self.imp().places_list.clone();
        let sidebar = self.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;

            let Some(row) = list.selected_row() else {
                // Delete must never reach the window (it would bin the
                // files selected in the unseen file list).
                if key == gdk::Key::Delete {
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            };
            let name = row.widget_name().to_string();

            // Delete key: remove a bookmark. On other rows it does nothing —
            // it mustn't reach the window, which would delete the files
            // selected in the (unseen) file list.
            if key == gdk::Key::Delete {
                if name.starts_with("bookmark:") {
                    remove_bookmark_row(&list, &row);
                }
                return glib::Propagation::Stop;
            }

            // Ctrl+Up/Down: reorder bookmarks
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            if !ctrl || !name.starts_with("bookmark:") {
                return glib::Propagation::Proceed;
            }
            if key != gdk::Key::Up && key != gdk::Key::Down {
                return glib::Propagation::Proceed;
            }

            // Find the neighbour bookmark in the requested direction
            let target_widget = if key == gdk::Key::Up {
                // Walk backwards to find previous bookmark row
                let mut prev = row.prev_sibling();
                loop {
                    match prev {
                        Some(ref w) if w.widget_name().starts_with("bookmark:") => break,
                        Some(ref w) => prev = w.prev_sibling(),
                        None => break,
                    }
                }
                prev
            } else {
                // Walk forward to find next bookmark row
                let mut next = row.next_sibling();
                loop {
                    match next {
                        Some(ref w) if w.widget_name().starts_with("bookmark:") => break,
                        Some(ref w) if w.widget_name() == "bookmark-separator" => {
                            next = w.next_sibling();
                        }
                        _ => break,
                    }
                }
                // Only proceed if we found another bookmark
                next.filter(|w| w.widget_name().starts_with("bookmark:"))
            };

            if target_widget.is_none() {
                // Announce boundary
                let msg = if key == gdk::Key::Up {
                    "Already at top"
                } else {
                    "Already at bottom"
                };
                list.announce(msg, gtk::AccessibleAnnouncementPriority::Medium);
                return glib::Propagation::Stop;
            }

            // Swap in the bookmarks file
            let mut bookmarks = match read_bookmarks() {
                Ok(bookmarks) => bookmarks,
                Err(e) => {
                    list.announce(&e, gtk::AccessibleAnnouncementPriority::High);
                    return glib::Propagation::Stop;
                }
            };
            let target_name = target_widget.as_ref().unwrap().widget_name().to_string();
            let cur_idx = bookmark_index(&bookmarks, name.strip_prefix("bookmark:").unwrap_or(""));
            let tgt_idx = bookmark_index(
                &bookmarks,
                target_name.strip_prefix("bookmark:").unwrap_or(""),
            );

            if let (Some(ci), Some(ti)) = (cur_idx, tgt_idx) {
                // Get the neighbour's display name for announcement
                let neighbour_display = bookmarks[ti].1.clone();
                let announcement = if key == gdk::Key::Up {
                    format!("Moved above {neighbour_display}")
                } else {
                    format!("Moved below {neighbour_display}")
                };

                bookmarks.swap(ci, ti);
                if let Err(e) = save_bookmarks(&bookmarks) {
                    list.announce(&e, gtk::AccessibleAnnouncementPriority::High);
                    return glib::Propagation::Stop;
                }

                // Refresh and re-focus — here, not by the refresh: the
                // moved row is to get focus with its "Moved" label, or
                // Orca reads its old one.
                sidebar.remake_bookmark_rows(false);
                refresh_all_bookmarks(Some(&sidebar));

                // Find and focus the moved bookmark row, with announcement as label
                let mut child = list.first_child();
                while let Some(widget) = child {
                    if widget.widget_name() == name {
                        if let Some(r) = widget.downcast_ref::<gtk::ListBoxRow>() {
                            r.update_property(&[gtk::accessible::Property::Label(&announcement)]);
                            list.select_row(Some(r));
                            r.grab_focus();
                            // Restore real label after Orca reads the announcement
                            let row_ref = r.clone();
                            // After the swap, the moved bookmark is at ti.
                            let current_display = bookmarks[ti].1.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(500),
                                move || {
                                    row_ref.update_property(&[gtk::accessible::Property::Label(
                                        &format!("Bookmark: {current_display}"),
                                    )]);
                                },
                            );
                        }
                        break;
                    }
                    child = widget.next_sibling();
                }
            }

            glib::Propagation::Stop
        });
        self.imp().places_list.add_controller(controller);
    }

    pub fn refresh_bookmarks(&self) {
        self.remake_bookmark_rows(true);
    }

    /// `keep_focus`: a bookmark's row with focus is about to be remade:
    /// keep focus on it (or near it, if it's gone).
    fn remake_bookmark_rows(&self, keep_focus: bool) {
        let list = &self.imp().places_list;
        let focused = list.focus_child().filter(|_| keep_focus).and_then(|c| {
            let name = c.widget_name().to_string();
            let row = c.downcast::<gtk::ListBoxRow>().ok()?;
            name.starts_with("bookmark:").then(|| (name, row.index()))
        });

        // Remove existing bookmark rows and bookmark separator
        let mut child = list.first_child();
        let mut to_remove = Vec::new();
        while let Some(widget) = child {
            let name = widget.widget_name().to_string();
            if name.starts_with("bookmark:") || name == "bookmark-separator" {
                to_remove.push(widget.clone());
            }
            child = widget.next_sibling();
        }
        for widget in to_remove {
            list.remove(&widget);
        }

        // Only local folders: other URIs (sftp://…) can't be opened here.
        let bookmarks = load_bookmarks();
        let local: Vec<(String, &String)> = bookmarks
            .iter()
            .filter_map(|(uri, name)| bookmark_path(uri).map(|p| (p, name)))
            .collect();
        if local.is_empty() {
            if let Some((name, index)) = focused {
                refocus_row(list, &name, index);
            }
            return;
        }

        let insert_pos = bookmarks_insert_pos(&row_names(list)) as i32;

        // Insert bookmark separator
        let sep_row = gtk::ListBoxRow::new();
        sep_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        sep_row.set_selectable(false);
        sep_row.set_activatable(false);
        sep_row.set_widget_name("bookmark-separator");
        list.insert(&sep_row, insert_pos);

        // Insert bookmark rows
        for (i, (path, display_name)) in local.iter().enumerate() {
            let path = path.as_str();

            let icon = gtk::Image::from_icon_name("folder-symbolic");
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(display_name.as_str())
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&label);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(&format!("bookmark:{path}"));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "Bookmark: {display_name}"
            ))]);

            list.insert(&row, insert_pos + 1 + i as i32);
        }
        if let Some((name, index)) = focused {
            refocus_row(list, &name, index);
        }
    }

    /// Refresh the volumes section in the sidebar using GIO VolumeMonitor.
    pub fn refresh_volumes(&self) {
        let list = &self.imp().places_list;

        // A drive's row with focus may be about to go (ejected): keep focus
        // in the sidebar, on the row that takes its place.
        let focused = list.focus_child().and_then(|c| {
            let name = c.widget_name().to_string();
            let row = c.downcast::<gtk::ListBoxRow>().ok()?;
            (name.starts_with("volume:") || name.starts_with("volume-unmounted:"))
                .then(|| (name, row.index()))
        });

        // Remove existing volume rows and volume separator
        let mut child = list.first_child();
        let mut to_remove = Vec::new();
        while let Some(widget) = child {
            let name = widget.widget_name().to_string();
            if name.starts_with("volume:")
                || name.starts_with("volume-unmounted:")
                || name == "volume-separator"
            {
                to_remove.push(widget.clone());
            }
            child = widget.next_sibling();
        }
        for widget in to_remove {
            list.remove(&widget);
        }

        let vm = gio::VolumeMonitor::get();
        let mut volume_rows: Vec<(String, String, String, bool)> = Vec::new(); // (widget_name, label, icon_name, can_eject)

        // Mounted volumes
        for mount in vm.mounts() {
            let name = mount.name().to_string();
            let root = mount.default_location();
            // One with no place on disk (a phone or network share without
            // GVfs's FUSE bridge) can't be opened here: leave it out, rather
            // than show a row that opens nothing and ejects the wrong one.
            let Some(path) = root.path().map(|p| p.to_string_lossy().to_string()) else {
                continue;
            };

            // Skip root filesystem and home — already in places
            if path == "/"
                || path
                    == dirs::home_dir()
                        .map(|h| h.to_string_lossy().to_string())
                        .unwrap_or_default()
            {
                continue;
            }

            let icon_name = "drive-removable-media-symbolic".to_string();

            let can_eject = mount.can_eject() || mount.can_unmount();
            volume_rows.push((format!("volume:{path}"), name, icon_name, can_eject));
        }

        // Unmounted volumes (that have no mount)
        for vol in vm.volumes() {
            if vol.get_mount().is_some() {
                continue; // already shown as mounted
            }
            let name = vol.name().to_string();
            let uuid = vol.uuid().map(|u| u.to_string()).unwrap_or_default();
            if uuid.is_empty() {
                continue;
            }
            let icon_name = "drive-removable-media-symbolic".to_string();

            volume_rows.push((
                format!("volume-unmounted:{uuid}"),
                format!("{name} (unmounted)"),
                icon_name,
                false,
            ));
        }

        if volume_rows.is_empty() {
            if let Some((name, index)) = focused {
                refocus_row(list, &name, index);
            }
            return;
        }

        let insert_pos = volumes_insert_pos(&row_names(list)) as i32;

        // Insert volume separator
        let sep_row = gtk::ListBoxRow::new();
        sep_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        sep_row.set_selectable(false);
        sep_row.set_activatable(false);
        sep_row.set_widget_name("volume-separator");
        list.insert(&sep_row, insert_pos);

        // Insert volume rows
        for (i, (widget_name, label, icon_name, can_eject)) in volume_rows.iter().enumerate() {
            let icon = gtk::Image::from_icon_name(icon_name.as_str());
            icon.set_pixel_size(16);

            let name_label = gtk::Label::builder()
                .label(label.as_str())
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&name_label);

            // Eject button for removable/unmountable volumes
            if *can_eject {
                let eject_btn = gtk::Button::from_icon_name("media-eject-symbolic");
                eject_btn.add_css_class("flat");
                eject_btn.set_tooltip_text(Some(&format!("Eject {label}")));
                eject_btn.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Eject {label}"
                ))]);

                // Find the mount path to identify which mount to eject
                let mount_path = widget_name
                    .strip_prefix("volume:")
                    .unwrap_or("")
                    .to_string();
                let label = label.clone();
                eject_btn.connect_clicked(move |btn| {
                    eject_mount_at(btn.upcast_ref(), &mount_path, &label);
                });

                row_box.append(&eject_btn);
            }

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(widget_name);
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "Volume: {label}"
            ))]);

            list.insert(&row, insert_pos + 1 + i as i32);
        }
        if let Some((name, index)) = focused {
            refocus_row(list, &name, index);
        }
    }

    /// Connect to GIO VolumeMonitor signals for dynamic updates.
    pub fn connect_volume_signals(&self) {
        let vm = &self.imp().volume_monitor;
        // Weakly: the monitor lives as long as the process, and would keep
        // every closed window's sidebar (and so the window) alive.
        let refresh = |sidebar: glib::WeakRef<WayfinderSidebar>| {
            move || {
                if let Some(sidebar) = sidebar.upgrade() {
                    sidebar.refresh_volumes();
                }
            }
        };
        let f = refresh(self.downgrade());
        vm.connect_mount_added(move |_, _| f());
        let f = refresh(self.downgrade());
        vm.connect_mount_removed(move |_, _| f());
        let f = refresh(self.downgrade());
        vm.connect_volume_added(move |_, _| f());
        let f = refresh(self.downgrade());
        vm.connect_volume_removed(move |_, _| f());
    }

    pub fn rebuild(&self) {
        let list = &self.imp().places_list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        self.populate_places();
    }

    pub fn connect_sidebar_edit_menu(&self, parent_window: &gtk::Window) {
        let click = gtk::GestureClick::new();
        click.set_button(3); // right click
        let sidebar = self.clone();
        let parent = parent_window.clone();
        let list = self.imp().places_list.clone();
        click.connect_pressed(move |gesture, _n, x, y| {
            // The Bin row has its own menu; a bookmark gets the bookmark menu.
            if let Some(row) = list.row_at_y(y as i32) {
                let name = row.widget_name().to_string();
                if name == "place:trash:///" {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    show_trash_menu(&sidebar, &parent, &row);
                    return;
                }
                if name.starts_with("bookmark:") {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    list.select_row(Some(&row));
                    show_bookmark_menu(&sidebar, &parent, &list, &row);
                    return;
                }
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);

            let popover = gtk::Popover::new();
            popover.set_parent(&list);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.set_has_arrow(false);
            let eject = list
                .row_at_y(y as i32)
                .and_then(|row| eject_item(&row, &popover));

            let btn = gtk::Button::with_label("Edit Sidebar");
            btn.add_css_class("flat");
            btn.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

            let s = sidebar.clone();
            let p = parent.clone();
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                s.show_edit_dialog(&p);
            });

            let menu = with_settings(btn.upcast_ref());
            if let Some(eject) = eject {
                menu.prepend(&eject);
            }
            popover.set_child(Some(&menu));
            popover.connect_closed(|pop| pop.unparent());
            focus_first_when_shown(&popover);
            popover.popup();
        });
        self.imp().places_list.add_controller(click);

        // Also handle Menu key and Shift+F10 for keyboard-triggered context menu
        let key_ctrl = gtk::EventControllerKey::new();
        let sidebar2 = self.clone();
        let parent2 = parent_window.clone();
        let list2 = self.imp().places_list.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;
            let is_menu = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK));

            if !is_menu {
                return glib::Propagation::Proceed;
            }

            // Check what row is focused — if it's a bookmark, let the delete key handler deal with it
            if let Some(row) = list2.selected_row() {
                let name = row.widget_name().to_string();
                if name.starts_with("bookmark:") {
                    show_bookmark_menu(&sidebar2, &parent2, &list2, &row);
                    return glib::Propagation::Stop;
                }
                // The Bin's own menu, as right-click gives (Empty Bin).
                if name == "place:trash:///" {
                    show_trash_menu(&sidebar2, &parent2, &row);
                    return glib::Propagation::Stop;
                }
            }

            // For all other rows, show "Edit Sidebar"
            let popover = gtk::Popover::new();
            let anchor = list2
                .selected_row()
                .map(|r| r.upcast::<gtk::Widget>())
                .unwrap_or_else(|| list2.clone().upcast());
            popover.set_parent(&anchor);
            popover.set_has_arrow(false);
            let eject = list2
                .selected_row()
                .and_then(|row| eject_item(&row, &popover));

            let btn = gtk::Button::with_label("Edit Sidebar");
            btn.add_css_class("flat");
            btn.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

            let s = sidebar2.clone();
            let p = parent2.clone();
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                s.show_edit_dialog(&p);
            });

            let menu = with_settings(btn.upcast_ref());
            if let Some(eject) = eject {
                menu.prepend(&eject);
            }
            popover.set_child(Some(&menu));
            popover.connect_closed(|pop| pop.unparent());
            focus_first_when_shown(&popover);
            popover.popup();
            glib::Propagation::Stop
        });
        self.imp().places_list.add_controller(key_ctrl);
    }

    pub fn show_edit_dialog(&self, parent: &gtk::Window) {
        let dlg = gtk::Window::builder()
            .title("Edit Sidebar")
            .modal(true)
            .transient_for(parent)
            .default_width(350)
            .default_height(450)
            .build();

        // Escape closes it, as Close (Cancel) does.
        let key_ctrl = gtk::EventControllerKey::new();
        let d = dlg.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);
        dlg.update_property(&[gtk::accessible::Property::Label("Edit sidebar places")]);

        let (vbox, edit_list, row_data) = places_editor();

        // Done button
        let done_btn = gtk::Button::with_label("Done");
        done_btn.set_halign(gtk::Align::End);
        done_btn.set_margin_top(8);
        vbox.append(&done_btn);

        dlg.set_child(Some(&vbox));

        let d = dlg.clone();
        done_btn.connect_clicked(move |_| {
            d.close();
            save_places(&row_data);
        });

        dlg.present();
        // Focus first row
        if let Some(first) = edit_list.row_at_index(0) {
            edit_list.select_row(Some(&first));
            first.grab_focus();
        }
    }

    fn populate_places(&self) {
        let list = &self.imp().places_list;

        // All available default places with their paths
        let mut all_places: Vec<(&str, &str, String)> = Vec::new();
        for place in PLACES {
            if let Some(path) = (place.dir_fn)() {
                all_places.push((place.name, place.icon, path.to_string_lossy().to_string()));
            }
        }
        all_places.push(("File System", "drive-harddisk-symbolic", "/".to_string()));

        // Determine order and visibility from config
        let config = crate::state::load_sidebar_config();

        let ordered: Vec<(&str, &str, String, bool)> = if let Some(ref config) = config {
            let mut result = Vec::new();
            // Config-ordered entries first
            for entry in config {
                if let Some((name, icon, path)) = all_places.iter().find(|(n, _, _)| *n == entry.id)
                {
                    result.push((*name, *icon, path.clone(), entry.visible));
                }
            }
            // Any new places not in config
            for (name, icon, path) in &all_places {
                if !result.iter().any(|(n, _, _, _)| n == name) {
                    result.push((name, icon, path.clone(), true));
                }
            }
            result
        } else {
            all_places
                .iter()
                .map(|(n, i, p)| (*n, *i, p.clone(), true))
                .collect()
        };

        for (name, icon_name, path, visible) in &ordered {
            if !visible {
                continue;
            }

            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(*name)
                .xalign(0.0)
                .hexpand(true)
                .build();

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(4)
                .margin_bottom(4)
                .margin_start(8)
                .margin_end(8)
                .build();
            row_box.append(&icon);
            row_box.append(&label);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&row_box));
            row.set_widget_name(&format!("place:{path}"));
            row.update_property(&[gtk::accessible::Property::Label(name)]);

            list.append(&row);
        }

        // Bookmarks from ~/.config/gtk-3.0/bookmarks
        self.refresh_bookmarks();

        // Mounted volumes and drives
        self.refresh_volumes();

        // Separator before Recent / Bin
        let separator_row = gtk::ListBoxRow::new();
        separator_row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        separator_row.set_selectable(false);
        separator_row.set_activatable(false);
        list.append(&separator_row);

        // Recent Files
        let icon = gtk::Image::from_icon_name("document-open-recent-symbolic");
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label("Recent")
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        row_box.append(&icon);
        row_box.append(&label);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_widget_name("place:recent:///");
        row.update_property(&[gtk::accessible::Property::Label("Recent files")]);
        list.append(&row);

        // Bin
        let icon = gtk::Image::from_icon_name("user-trash-symbolic");
        icon.set_pixel_size(16);
        let label = gtk::Label::builder()
            .label("Bin")
            .xalign(0.0)
            .hexpand(true)
            .build();
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        row_box.append(&icon);
        row_box.append(&label);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_widget_name("place:trash:///");
        row.update_property(&[gtk::accessible::Property::Label("Bin")]);
        list.append(&row);
    }
}

impl Default for WayfinderSidebar {
    fn default() -> Self {
        Self::new()
    }
}

/// The list of places that can be shown in the sidebar, each with a check
/// box (Ctrl+Up/Down and Ctrl+Shift+Home/End reorder it), with its
/// instructions, in a box: for Edit Sidebar and Preferences. Also the list,
/// and the places as they're edited (`save_places` keeps them).
pub fn places_editor() -> (gtk::Box, gtk::ListBox, PlacesEdit) {
    // Load current config
    let config = crate::state::load_sidebar_config();

    // All available default places with their paths
    let mut all_places: Vec<(&str, &str, String)> = Vec::new();
    for place in PLACES {
        if let Some(path) = (place.dir_fn)() {
            all_places.push((place.name, place.icon, path.to_string_lossy().to_string()));
        }
    }
    all_places.push(("File System", "drive-harddisk-symbolic", "/".to_string()));

    // Determine order and visibility
    struct EditRow {
        id: String,
        icon: String,
        visible: bool,
    }

    let rows: Vec<EditRow> = if let Some(ref config) = config {
        let mut result: Vec<EditRow> = Vec::new();
        for entry in config {
            if let Some((_name, icon, _path)) = all_places.iter().find(|(n, _, _)| *n == entry.id) {
                result.push(EditRow {
                    id: entry.id.clone(),
                    icon: icon.to_string(),
                    visible: entry.visible,
                });
            }
        }
        // Add any places not in config
        for (name, icon, _) in &all_places {
            if !result.iter().any(|r| r.id == *name) {
                result.push(EditRow {
                    id: name.to_string(),
                    icon: icon.to_string(),
                    visible: true,
                });
            }
        }
        result
    } else {
        all_places
            .iter()
            .map(|(name, icon, _)| EditRow {
                id: name.to_string(),
                icon: icon.to_string(),
                visible: true,
            })
            .collect()
    };

    // Build the dialog UI
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let instructions = gtk::Label::builder()
        .label("Toggle places on or off. Use Ctrl+Up/Down to reorder.")
        .xalign(0.0)
        .wrap(true)
        .build();
    vbox.append(&instructions);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let edit_list = gtk::ListBox::new();
    edit_list.set_selection_mode(gtk::SelectionMode::Single);
    edit_list.update_property(&[gtk::accessible::Property::Label("Sidebar places")]);

    let row_data: std::rc::Rc<std::cell::RefCell<Vec<(String, bool)>>> = std::rc::Rc::new(
        std::cell::RefCell::new(rows.iter().map(|r| (r.id.clone(), r.visible)).collect()),
    );

    for row_info in &rows {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);

        let check = gtk::CheckButton::new();
        check.set_active(row_info.visible);
        check.update_property(&[gtk::accessible::Property::Label(&format!(
            "Show {}",
            row_info.id
        ))]);

        let icon = gtk::Image::from_icon_name(&row_info.icon);
        icon.set_pixel_size(16);

        let label = gtk::Label::builder()
            .label(&row_info.id)
            .xalign(0.0)
            .hexpand(true)
            .build();

        hbox.append(&check);
        hbox.append(&icon);
        hbox.append(&label);

        let list_row = gtk::ListBoxRow::new();
        list_row.set_child(Some(&hbox));
        list_row.set_widget_name(&row_info.id);
        // The row is what has focus, so it says whether it's shown.
        let row_label =
            |id: &str, shown: bool| format!("{id}, {}", if shown { "shown" } else { "hidden" });
        list_row.update_property(&[gtk::accessible::Property::Label(&row_label(
            &row_info.id,
            row_info.visible,
        ))]);

        // Sync checkbox with data
        let data = row_data.clone();
        let id = row_info.id.clone();
        let row_for_label = list_row.clone();
        let dlg_for_toggle = edit_list.clone();
        check.connect_toggled(move |cb| {
            let mut d = data.borrow_mut();
            if let Some(entry) = d.iter_mut().find(|(name, _)| name == &id) {
                entry.1 = cb.is_active();
            }
            let text = row_label(&id, cb.is_active());
            row_for_label.update_property(&[gtk::accessible::Property::Label(&text)]);
            dlg_for_toggle.announce(&text, gtk::AccessibleAnnouncementPriority::Medium);
        });

        edit_list.append(&list_row);
    }

    // Enter or Space on a row ticks or unticks it.
    edit_list.connect_row_activated(|_, row| {
        if let Some(check) = row
            .child()
            .and_then(|hbox| hbox.first_child())
            .and_downcast::<gtk::CheckButton>()
        {
            check.set_active(!check.is_active());
        }
    });

    scrolled.set_child(Some(&edit_list));
    vbox.append(&scrolled);

    // Keyboard controller for reordering with announcements
    let key_ctrl = gtk::EventControllerKey::new();
    // Capture: the ListBox's own Ctrl+arrow bindings would take the keys.
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let list_for_keys = edit_list.clone();
    let data_for_keys = row_data.clone();
    let dlg_for_keys = edit_list.clone();
    key_ctrl.connect_key_pressed(move |_, key, _, mods| {
        use gtk::gdk;
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);

        if !ctrl {
            return glib::Propagation::Proceed;
        }

        let is_move = matches!(
            (key, shift),
            (gdk::Key::Up | gdk::Key::Down, false) | (gdk::Key::Home | gdk::Key::End, true)
        );
        if !is_move {
            return glib::Propagation::Proceed;
        }

        let Some(selected) = list_for_keys.selected_row() else {
            return glib::Propagation::Proceed;
        };
        let idx = selected.index();
        let n = data_for_keys.borrow().len() as i32;

        // Check boundaries and announce if can't move
        match (key, shift) {
            (gdk::Key::Up, false) if idx == 0 => {
                dlg_for_keys.announce(
                    "Already at top",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                return glib::Propagation::Stop;
            }
            (gdk::Key::Down, false) if idx >= n - 1 => {
                dlg_for_keys.announce(
                    "Already at bottom",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                return glib::Propagation::Stop;
            }
            (gdk::Key::Home, true) if idx == 0 => {
                dlg_for_keys.announce(
                    "Already at top",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                return glib::Propagation::Stop;
            }
            (gdk::Key::End, true) if idx >= n - 1 => {
                dlg_for_keys.announce(
                    "Already at bottom",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                return glib::Propagation::Stop;
            }
            _ => {}
        }

        let new_idx = match (key, shift) {
            (gdk::Key::Up, false) => idx - 1,
            (gdk::Key::Down, false) => idx + 1,
            (gdk::Key::Home, true) => 0,
            (gdk::Key::End, true) => n - 1,
            _ => unreachable!(),
        };

        // Compute the announcement before rebuilding
        let announcement = {
            let d = data_for_keys.borrow();
            let neighbour_name = if key == gdk::Key::Up || (key == gdk::Key::Home && shift) {
                // Will move above — neighbour is the one currently at new_idx
                d.get(new_idx as usize)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default()
            } else {
                // Will move below — neighbour is the one currently at new_idx
                d.get(new_idx as usize)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default()
            };
            match (key, shift) {
                (gdk::Key::Home, true) => "Moved to top".to_string(),
                (gdk::Key::End, true) => "Moved to bottom".to_string(),
                (gdk::Key::Up, _) => format!("Moved above {neighbour_name}"),
                (gdk::Key::Down, _) => format!("Moved below {neighbour_name}"),
                _ => String::new(),
            }
        };

        // Move in data: remove from old position, insert at new
        {
            let mut d = data_for_keys.borrow_mut();
            let item = d.remove(idx as usize);
            d.insert(new_idx as usize, item);
        }

        // Rebuild the list visually to keep everything in sync
        while let Some(child) = list_for_keys.first_child() {
            list_for_keys.remove(&child);
        }
        let data = data_for_keys.borrow();
        for (i, (id, vis)) in data.iter().enumerate() {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(8);
            hbox.set_margin_end(8);

            let check = gtk::CheckButton::new();
            check.set_active(*vis);
            check.update_property(&[gtk::accessible::Property::Label(&format!("Show {id}"))]);

            let icon_name = PLACES
                .iter()
                .find(|p| p.name == id)
                .map(|p| p.icon)
                .unwrap_or(if id == "File System" {
                    "drive-harddisk-symbolic"
                } else {
                    "folder-symbolic"
                });
            let icon = gtk::Image::from_icon_name(icon_name);
            icon.set_pixel_size(16);

            let label = gtk::Label::builder()
                .label(id.as_str())
                .xalign(0.0)
                .hexpand(true)
                .build();

            hbox.append(&check);
            hbox.append(&icon);
            hbox.append(&label);

            let list_row = gtk::ListBoxRow::new();
            list_row.set_child(Some(&hbox));
            list_row.set_widget_name(id);

            // The row says whether it's shown, as when the dialog opened.
            let row_label =
                |id: &str, shown: bool| format!("{id}, {}", if shown { "shown" } else { "hidden" });
            // For the moved row, set the announcement as the label
            // so Orca reads it instead of the plain name
            if i == new_idx as usize {
                list_row.update_property(&[gtk::accessible::Property::Label(&announcement)]);
            } else {
                list_row.update_property(&[gtk::accessible::Property::Label(&row_label(id, *vis))]);
            }

            // Sync checkbox
            let data_ref = data_for_keys.clone();
            let id_clone = id.clone();
            let row_for_label = list_row.clone();
            let dlg_for_toggle = dlg_for_keys.clone();
            check.connect_toggled(move |cb| {
                let mut d = data_ref.borrow_mut();
                if let Some(entry) = d.iter_mut().find(|(name, _)| name == &id_clone) {
                    entry.1 = cb.is_active();
                }
                let text = row_label(&id_clone, cb.is_active());
                row_for_label.update_property(&[gtk::accessible::Property::Label(&text)]);
                dlg_for_toggle.announce(&text, gtk::AccessibleAnnouncementPriority::Medium);
            });

            list_for_keys.append(&list_row);

            if i == new_idx as usize {
                list_for_keys.select_row(Some(&list_row));
                // Restore the real label after Orca has read the announcement
                let row_ref = list_row.clone();
                // Read when it runs: the box may be ticked meanwhile.
                let check_now = check.clone();
                let id_now = id.clone();
                let row_to_focus = list_row.clone();
                glib::idle_add_local_once(move || {
                    row_to_focus.grab_focus();
                });
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    row_ref.update_property(&[gtk::accessible::Property::Label(&row_label(
                        &id_now,
                        check_now.is_active(),
                    ))]);
                });
            }
        }
        drop(data);

        glib::Propagation::Stop
    });
    edit_list.add_controller(key_ctrl);

    (vbox, edit_list, row_data)
}

/// Places as edited: (name, shown), in order.
pub type PlacesEdit = std::rc::Rc<std::cell::RefCell<Vec<(String, bool)>>>;

/// Keep the places as edited, and show them in every window's sidebar.
pub fn save_places(edit: &PlacesEdit) {
    let entries: Vec<crate::state::SidebarEntry> = edit
        .borrow()
        .iter()
        .map(|(id, vis)| crate::state::SidebarEntry {
            id: id.clone(),
            visible: *vis,
        })
        .collect();
    crate::state::save_sidebar_config(&entries);
    for other in live_sidebars() {
        other.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::{bookmark_path, bookmarks_insert_pos, read_bookmarks_from, volumes_insert_pos};

    #[test]
    fn bookmarks_without_a_name_are_saved_without_one() {
        let text = "file:///home/u/My%20Docs\nfile:///home/u/Work Projects";
        assert_eq!(super::bookmarks_text(&super::parse_bookmarks(text)), text);
    }

    #[test]
    fn a_bookmarks_file_that_cant_be_read_isnt_taken_for_none() {
        let d = std::env::temp_dir().join(format!("wayfinder-bm-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // Not there yet: no bookmarks, fine to write.
        assert_eq!(read_bookmarks_from(&d.join("none")), Ok(Vec::new()));
        // There, but not UTF-8: an error, so it isn't written over.
        std::fs::write(d.join("latin1"), b"file:///home/u/Caf%C3%A9 Caf\xe9\n").unwrap();
        assert!(read_bookmarks_from(&d.join("latin1")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bookmarks_go_after_the_places_shown() {
        // File System hidden: still above the drives and Recent / Bin.
        let rows = names(&[
            "place:/home/u",
            "place:/home/u/Music",
            "volume-separator",
            "volume:/media/usb",
            "GtkListBoxRow",
            "place:recent:///",
            "place:trash:///",
        ]);
        assert_eq!(bookmarks_insert_pos(&rows), 2);
        // While the list is first built.
        assert_eq!(
            bookmarks_insert_pos(&names(&["place:/home/u", "place:/"])),
            2
        );
    }

    #[test]
    fn drives_go_above_recent_and_bin() {
        let rows = names(&[
            "place:/home/u",
            "bookmark-separator",
            "bookmark:/data",
            "GtkListBoxRow",
            "place:recent:///",
            "place:trash:///",
        ]);
        // Before the separator above Recent.
        assert_eq!(volumes_insert_pos(&rows), 3);
        assert_eq!(volumes_insert_pos(&names(&["place:/home/u"])), 1);
    }

    #[test]
    fn bookmark_uris_are_decoded() {
        assert_eq!(
            bookmark_path("file:///home/u/My%20Docs").as_deref(),
            Some("/home/u/My Docs")
        );
        assert_eq!(
            bookmark_path("file:///home/u/Caf%C3%A9").as_deref(),
            Some("/home/u/Café")
        );
        assert_eq!(bookmark_path("sftp://host/dir"), None);
    }
}
