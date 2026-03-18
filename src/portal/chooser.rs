use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::file_model::DirectoryModel;
use wayfinder::file_object::FileObject;
use wayfinder::navigation::NavigationState;
use wayfinder::sidebar::WayfinderSidebar;
use wayfinder::views::{WayfinderGridView, WayfinderListView};

pub struct ChooserResult {
    pub uris: Vec<String>,
    pub cancelled: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn show_chooser(
    title: &str,
    accept_label: &str,
    is_save: bool,
    _multiple: bool,
    directory_mode: bool,
    current_folder: Option<&str>,
    current_name: Option<&str>,
    _filters: Vec<(String, Vec<String>)>,
    result_tx: tokio::sync::oneshot::Sender<ChooserResult>,
) {
    let model = Rc::new(DirectoryModel::new());
    let selection = gtk::SingleSelection::new(Some(model.filter_model.clone()));
    selection.set_autoselect(false); // Don't auto-select — user must deliberately choose

    let list_view = WayfinderListView::new();
    list_view.set_model(&selection);

    let grid_view = WayfinderGridView::new();

    let nav = Rc::new(RefCell::new(NavigationState::new(
        current_folder
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| "/".into()),
    )));

    let _current_view: Rc<Cell<ViewMode>> = Rc::new(Cell::new(ViewMode::List));
    let result_tx = Rc::new(RefCell::new(Some(result_tx)));

    // Build the dialog window
    let dlg = gtk::Window::builder()
        .title(title)
        .default_width(800)
        .default_height(500)
        .modal(true)
        .build();

    // Header bar
    let header = gtk::HeaderBar::new();

    let back_btn = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Go back")
        .sensitive(false)
        .build();
    back_btn.update_property(&[gtk::accessible::Property::Label("Go back")]);

    let forward_btn = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Go forward")
        .sensitive(false)
        .build();
    forward_btn.update_property(&[gtk::accessible::Property::Label("Go forward")]);

    let up_btn = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Go to parent directory")
        .build();
    up_btn.update_property(&[gtk::accessible::Property::Label("Go to parent directory")]);

    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_box.add_css_class("linked");
    nav_box.append(&back_btn);
    nav_box.append(&forward_btn);
    header.pack_start(&nav_box);
    header.pack_start(&up_btn);

    // Location combo with recent directories
    let location_entry = gtk::Entry::builder()
        .hexpand(true)
        .placeholder_text("Path")
        .build();
    location_entry.update_property(&[
        gtk::accessible::Property::Label("Location"),
    ]);
    header.set_title_widget(Some(&location_entry));

    dlg.set_titlebar(Some(&header));

    // View stack
    let view_stack = gtk::Stack::new();
    view_stack.set_transition_type(gtk::StackTransitionType::None);
    view_stack.add_named(grid_view.widget(), Some("grid"));
    view_stack.add_named(list_view.widget(), Some("list"));
    view_stack.set_visible_child_name("list");

    // Sidebar
    let sidebar = WayfinderSidebar::new();
    let sidebar_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideRight)
        .reveal_child(true)
        .child(sidebar.widget())
        .build();

    // Bottom bar with filename entry (for Save) and accept/cancel buttons
    let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom_bar.set_margin_top(8);
    bottom_bar.set_margin_bottom(8);
    bottom_bar.set_margin_start(8);
    bottom_bar.set_margin_end(8);

    let filename_entry = if is_save {
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .placeholder_text("Filename")
            .text(current_name.unwrap_or(""))
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("File name")]);
        bottom_bar.append(&entry);
        Some(entry)
    } else {
        // For Open, show a read-only label that shows the selected file
        let label = gtk::Label::builder()
            .label("No file selected")
            .hexpand(true)
            .xalign(0.0)
            .build();
        label.update_property(&[gtk::accessible::Property::Label("Selected file")]);
        bottom_bar.append(&label);
        None
    };

    let cancel_btn = gtk::Button::with_label("Cancel");
    cancel_btn.update_property(&[gtk::accessible::Property::Label("Cancel")]);

    let accept_btn = gtk::Button::with_label(accept_label);
    accept_btn.add_css_class("suggested-action");
    accept_btn.update_property(&[gtk::accessible::Property::Label(accept_label)]);

    bottom_bar.append(&cancel_btn);
    bottom_bar.append(&accept_btn);

    // Status label
    let status_label = gtk::Label::builder()
        .xalign(0.0)
        .margin_start(8)
        .build();

    // Layout
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.append(&view_stack);
    content_box.append(&status_label);
    content_box.append(&bottom_bar);

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&sidebar_revealer)
        .end_child(&content_box)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(180)
        .build();

    dlg.set_child(Some(&paned));

    // Wire ColumnView sorter
    let columns = list_view.column_view().columns();
    if let Some(first_col) = columns.item(0).and_downcast::<gtk::ColumnViewColumn>() {
        list_view
            .column_view()
            .sort_by_column(Some(&first_col), gtk::SortType::Ascending);
    }
    if let Some(cv_sorter) = list_view.column_view().sorter() {
        model.set_sorter(Some(&cv_sorter));
    }

    // Column navigation for list view (Left/Right to read columns, Tab to escape)
    let col_names: &[&str] = &["Name", "Size", "Date Modified", "Kind"];
    let current_column: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    {
        let col_controller = gtk::EventControllerKey::new();
        col_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cc = current_column.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        let le = location_entry.clone();
        col_controller.connect_key_pressed(move |_ctrl, key, _code, mods| {
            use gtk::gdk;
            if key == gdk::Key::Tab && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                le.grab_focus();
                glib::Propagation::Stop
            } else if key == gdk::Key::ISO_Left_Tab
                || (key == gdk::Key::Tab && mods.contains(gdk::ModifierType::SHIFT_MASK))
            {
                le.grab_focus();
                glib::Propagation::Stop
            } else if key == gdk::Key::Left && !mods.contains(gdk::ModifierType::ALT_MASK) {
                let col = cc.get();
                if col > 0 {
                    cc.set(col - 1);
                    if let Some(item) = sel.selected_item() {
                        if let Some(file) = item.downcast_ref::<FileObject>() {
                            let value = match cc.get() {
                                0 => file.name(),
                                1 => file.size_display(),
                                2 => file.modified_display(),
                                3 => file.file_type_name(),
                                _ => String::new(),
                            };
                            d.announce(
                                &format!("{}: {}", col_names[cc.get()], value),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                    }
                }
                glib::Propagation::Stop
            } else if key == gdk::Key::Right && !mods.contains(gdk::ModifierType::ALT_MASK) {
                let col = cc.get();
                if col + 1 < col_names.len() {
                    cc.set(col + 1);
                    if let Some(item) = sel.selected_item() {
                        if let Some(file) = item.downcast_ref::<FileObject>() {
                            let value = match cc.get() {
                                0 => file.name(),
                                1 => file.size_display(),
                                2 => file.modified_display(),
                                3 => file.file_type_name(),
                                _ => String::new(),
                            };
                            d.announce(
                                &format!("{}: {}", col_names[cc.get()], value),
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                    }
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        list_view.column_view().add_controller(col_controller);
    }

    // Announce column value on selection change in list view
    {
        let cc = current_column.clone();
        let d = dlg.clone();
        selection.connect_selected_item_notify(move |sel| {
            let col = cc.get();
            if let Some(item) = sel.selected_item() {
                if let Some(file) = item.downcast_ref::<FileObject>() {
                    let value = match col {
                        0 => file.name(),
                        1 => file.size_display(),
                        2 => file.modified_display(),
                        3 => file.file_type_name(),
                        _ => String::new(),
                    };
                    d.announce(
                        &format!("{}: {}", col_names[col], value),
                        AccessibleAnnouncementPriority::Medium,
                    );
                }
            }
        });
    }

    // Selected file tracking — Space selects, doesn't auto-select on navigation
    let selected_file: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // -- Load initial directory --
    let initial_path = nav.borrow().current().to_string_lossy().to_string();
    load_dir(
        &model,
        &initial_path,
        &location_entry,
        &status_label,
        &nav,
        &back_btn,
        &forward_btn,
        &up_btn,
        &selection,
        &dlg,
    );
    list_view.grab_focus();

    // -- Navigation button handlers --
    {
        let nav = nav.clone();
        let model = model.clone();
        let le = location_entry.clone();
        let sl = status_label.clone();
        let bb = back_btn.clone();
        let fb = forward_btn.clone();
        let ub = up_btn.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        back_btn.connect_clicked(move |_| {
            if let Some(path) = nav.borrow_mut().go_back().cloned() {
                load_dir(&model, &path.to_string_lossy(), &le, &sl, &nav, &bb, &fb, &ub, &sel, &d);
            }
        });
    }
    {
        let nav = nav.clone();
        let model = model.clone();
        let le = location_entry.clone();
        let sl = status_label.clone();
        let bb = back_btn.clone();
        let fb = forward_btn.clone();
        let ub = up_btn.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        forward_btn.connect_clicked(move |_| {
            if let Some(path) = nav.borrow_mut().go_forward().cloned() {
                load_dir(&model, &path.to_string_lossy(), &le, &sl, &nav, &bb, &fb, &ub, &sel, &d);
            }
        });
    }
    {
        let nav = nav.clone();
        let model = model.clone();
        let le = location_entry.clone();
        let sl = status_label.clone();
        let bb = back_btn.clone();
        let fb = forward_btn.clone();
        let ub = up_btn.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        up_btn.connect_clicked(move |_| {
            if let Some(parent) = nav.borrow().go_up() {
                nav.borrow_mut().navigate_to(parent.clone());
                load_dir(&model, &parent.to_string_lossy(), &le, &sl, &nav, &bb, &fb, &ub, &sel, &d);
            }
        });
    }

    // -- Sidebar navigation --
    {
        let nav = nav.clone();
        let model = model.clone();
        let le = location_entry.clone();
        let sl = status_label.clone();
        let bb = back_btn.clone();
        let fb = forward_btn.clone();
        let ub = up_btn.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        sidebar.connect_place_activated(move |path| {
            if !path.starts_with("trash:") {
                nav.borrow_mut().navigate_to(PathBuf::from(&path));
                load_dir(&model, &path, &le, &sl, &nav, &bb, &fb, &ub, &sel, &d);
            }
        });
    }

    // -- Space to select file (deliberate selection) --
    let space_controller = gtk::EventControllerKey::new();
    space_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let sel_for_space = selection.clone();
    let sf = selected_file.clone();
    let fn_entry = filename_entry.clone();
    let d = dlg.clone();
    space_controller.connect_key_pressed(move |_ctrl, key, _code, _mods| {
        if key == gtk::gdk::Key::space {
            if let Some(item) = sel_for_space.selected_item() {
                if let Some(file) = item.downcast_ref::<FileObject>() {
                    if !file.is_directory() || directory_mode {
                        *sf.borrow_mut() = Some(file.path());
                        if let Some(ref entry) = fn_entry {
                            entry.set_text(&file.name());
                        }
                        d.announce(
                            &format!("Selected {}", file.name()),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                }
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    list_view.column_view().add_controller(space_controller);

    // Same for grid view
    let space_controller2 = gtk::EventControllerKey::new();
    space_controller2.set_propagation_phase(gtk::PropagationPhase::Capture);
    let sel_for_space2 = selection.clone();
    let sf2 = selected_file.clone();
    let fn_entry2 = filename_entry.clone();
    let d2 = dlg.clone();
    space_controller2.connect_key_pressed(move |_ctrl, key, _code, _mods| {
        if key == gtk::gdk::Key::space {
            if let Some(item) = sel_for_space2.selected_item() {
                if let Some(file) = item.downcast_ref::<FileObject>() {
                    if !file.is_directory() || directory_mode {
                        *sf2.borrow_mut() = Some(file.path());
                        if let Some(ref entry) = fn_entry2 {
                            entry.set_text(&file.name());
                        }
                        d2.announce(
                            &format!("Selected {}", file.name()),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                }
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    grid_view.grid_view().add_controller(space_controller2);

    // -- Enter opens folders, doesn't select files --
    let enter_controller = gtk::EventControllerKey::new();
    enter_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let sel_for_enter = selection.clone();
    let nav_for_enter = nav.clone();
    let model_for_enter = model.clone();
    let le2 = location_entry.clone();
    let sl2 = status_label.clone();
    let bb2 = back_btn.clone();
    let fb2 = forward_btn.clone();
    let ub2 = up_btn.clone();
    let d3 = dlg.clone();
    enter_controller.connect_key_pressed(move |_ctrl, key, _code, _mods| {
        if key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter {
            if let Some(item) = sel_for_enter.selected_item() {
                if let Some(file) = item.downcast_ref::<FileObject>() {
                    if file.is_directory() {
                        let path = file.path();
                        nav_for_enter.borrow_mut().navigate_to(PathBuf::from(&path));
                        load_dir(
                            &model_for_enter,
                            &path,
                            &le2, &sl2, &nav_for_enter,
                            &bb2, &fb2, &ub2,
                            &sel_for_enter, &d3,
                        );
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        } else {
            glib::Propagation::Proceed
        }
    });
    list_view.column_view().add_controller(enter_controller);

    // -- Cancel --
    let tx_cancel = result_tx.clone();
    let d_cancel = dlg.clone();
    cancel_btn.connect_clicked(move |_| {
        if let Some(tx) = tx_cancel.borrow_mut().take() {
            let _ = tx.send(ChooserResult {
                uris: vec![],
                cancelled: true,
            });
        }
        d_cancel.close();
    });

    // -- Accept --
    let tx_accept = result_tx.clone();
    let sf_accept = selected_file.clone();
    let fn_accept = filename_entry.clone();
    let model_accept = model.clone();
    let d_accept = dlg.clone();
    accept_btn.connect_clicked(move |_| {
        let uri = if is_save {
            // For Save: use the filename entry text combined with current directory
            if let Some(ref entry) = fn_accept {
                let name = entry.text().to_string();
                if !name.is_empty() {
                    let dir = model_accept.current_path();
                    let full_path = if dir == "/" {
                        format!("/{}", name)
                    } else {
                        format!("{}/{}", dir, name)
                    };
                    Some(format!("file://{}", full_path))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            // For Open: use the space-selected file
            sf_accept
                .borrow()
                .as_ref()
                .map(|p| format!("file://{}", p))
        };

        if let Some(uri) = uri {
            if let Some(tx) = tx_accept.borrow_mut().take() {
                let _ = tx.send(ChooserResult {
                    uris: vec![uri],
                    cancelled: false,
                });
            }
            d_accept.close();
        } else {
            d_accept.announce(
                "No file selected",
                AccessibleAnnouncementPriority::High,
            );
        }
    });

    // Close window = cancel
    let tx_close = result_tx.clone();
    dlg.connect_close_request(move |_| {
        if let Some(tx) = tx_close.borrow_mut().take() {
            let _ = tx.send(ChooserResult {
                uris: vec![],
                cancelled: true,
            });
        }
        glib::Propagation::Proceed
    });

    dlg.present();
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum ViewMode {
    Grid,
    List,
}

#[allow(clippy::too_many_arguments)]
fn load_dir(
    model: &DirectoryModel,
    path: &str,
    location_entry: &gtk::Entry,
    status_label: &gtk::Label,
    nav: &Rc<RefCell<NavigationState>>,
    back_btn: &gtk::Button,
    forward_btn: &gtk::Button,
    up_btn: &gtk::Button,
    selection: &gtk::SingleSelection,
    window: &gtk::Window,
) {
    match model.load_directory(path) {
        Ok(_) => {
            location_entry.set_text(path);
            back_btn.set_sensitive(nav.borrow().can_go_back());
            forward_btn.set_sensitive(nav.borrow().can_go_forward());
            up_btn.set_sensitive(path != "/");
            let count = model.item_count();
            status_label.set_text(&format!("{} items", count));
            selection.set_selected(gtk::INVALID_LIST_POSITION); // Clear selection
            let dir_name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            window.announce(
                &format!("Opened {}, {} items", dir_name, count),
                AccessibleAnnouncementPriority::Medium,
            );
        }
        Err(e) => {
            window.announce(
                &format!("Error: {}", e),
                AccessibleAnnouncementPriority::High,
            );
        }
    }
}
