use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::file_model::DirectoryModel;
use wayfinder::file_object::FileObject;
use wayfinder::navigation::NavigationState;
use wayfinder::sidebar::WayfinderSidebar;
use wayfinder::views::{WayfinderGridView, WayfinderListView};

use crate::options::{self, Choice, Filter, Memory};

pub struct ChooserResult {
    pub uris: Vec<String>,
    pub cancelled: bool,
    /// The app's extra choices, as `(id, value)`.
    pub choices: Vec<(String, String)>,
    /// The file type chosen, if the app offered some.
    pub current_filter: Option<Filter>,
}

/// What the app asked for.
pub struct ChooserSpec {
    pub title: String,
    pub accept_label: String,
    pub is_save: bool,
    pub directory_mode: bool,
    pub multiple: bool,
    pub current_folder: Option<String>,
    pub current_name: Option<String>,
    pub filters: Vec<Filter>,
    pub current_filter: Option<Filter>,
    pub choices: Vec<Choice>,
}

/// A control made for one of the app's choices.
enum ChoiceControl {
    Check(String, gtk::CheckButton),
    List(String, gtk::DropDown, Vec<String>),
}

/// Tab order, top to bottom: File name, the type (Save as type / Files of
/// type), the app's own choices, Location (Up/Down: recent folders), More
/// options; then, with More options expanded, the places sidebar, Back,
/// Forward, Up and the files; then Cancel and the app's Save/Open.
pub fn show_chooser(spec: ChooserSpec, result_tx: tokio::sync::oneshot::Sender<ChooserResult>) {
    let ChooserSpec {
        title,
        accept_label,
        is_save,
        directory_mode,
        multiple,
        current_folder,
        current_name,
        filters,
        current_filter,
        choices,
    } = spec;
    // Several files: only when opening files.
    let multiple = multiple && !is_save && !directory_mode;
    let memory = Memory::load();

    // -- File types --
    let (filters, first_filter) = if directory_mode {
        (Vec::new(), 0)
    } else {
        options::filter_list(filters, current_filter)
    };
    let filters = Rc::new(filters);
    let active_filter: Rc<Cell<Option<usize>>> =
        Rc::new(Cell::new((!filters.is_empty()).then_some(first_filter)));

    let model = Rc::new(DirectoryModel::new());
    let type_filter = {
        let filters = filters.clone();
        let active = active_filter.clone();
        gtk::CustomFilter::new(move |item| {
            let Some(file) = item.downcast_ref::<FileObject>() else {
                return true;
            };
            if file.is_directory() {
                return true;
            }
            match active.get().and_then(|i| filters.get(i)) {
                Some(f) => options::filter_matches(f, &file.name(), &file.mime_type()),
                None => true,
            }
        })
    };
    let shown =
        gtk::FilterListModel::new(Some(model.filter_model.clone()), Some(type_filter.clone()));
    let selection = gtk::SingleSelection::new(Some(shown));
    selection.set_autoselect(false); // Don't auto-select — user must deliberately choose

    let list_view = WayfinderListView::new();

    let grid_view = WayfinderGridView::new();

    let nav = Rc::new(RefCell::new(NavigationState::new(
        current_folder
            .as_deref()
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| "/".into()),
    )));

    let result_tx = Rc::new(RefCell::new(Some(result_tx)));

    // Build the dialog window
    let dlg = gtk::Window::builder()
        .title(title.as_str())
        .default_width(800)
        .default_height(if memory.expanded { 560 } else { -1 })
        .modal(true)
        .build();
    dlg.set_titlebar(Some(&gtk::HeaderBar::new()));

    // -- The fields at the top --
    let top = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    let mut row = 0;
    let field = |grid: &gtk::Grid, row: &mut i32, label: &str, widget: &gtk::Widget, name: &str| {
        let l = gtk::Label::builder()
            .label(label)
            .use_underline(true)
            .xalign(1.0)
            .build();
        l.set_mnemonic_widget(Some(widget));
        widget.update_property(&[gtk::accessible::Property::Label(name)]);
        widget.set_hexpand(true);
        grid.attach(&l, 0, *row, 1, 1);
        grid.attach(widget, 1, *row, 1, 1);
        *row += 1;
    };

    let suggested_name: Rc<str> = Rc::from(current_name.as_deref().unwrap_or(""));
    let name_entry = gtk::Entry::builder().text(&*suggested_name).build();
    let (name_label, name_name) = if directory_mode {
        ("Folder _name:", "Folder name")
    } else {
        ("File _name:", "File name")
    };
    field(
        &top,
        &mut row,
        name_label,
        name_entry.upcast_ref(),
        name_name,
    );
    // Back to the app's name: shown only once the name differs from it.
    let reset_btn = gtk::Button::with_mnemonic("_Reset file name");
    reset_btn.update_property(&[gtk::accessible::Property::Label("Reset file name")]);
    reset_btn.set_visible(false);
    top.attach(&reset_btn, 2, row - 1, 1, 1);

    let type_dropdown = (!filters.is_empty()).then(|| {
        let names: Vec<&str> = filters.iter().map(|f| f.name.as_str()).collect();
        let dd = gtk::DropDown::from_strings(&names);
        dd.set_selected(u32::try_from(first_filter).unwrap_or(0));
        let (label, name) = if is_save {
            ("Save as _type:", "Save as type")
        } else {
            ("Files of _type:", "Files of type")
        };
        field(&top, &mut row, label, dd.upcast_ref(), name);
        dd
    });

    let choice_controls: Rc<Vec<ChoiceControl>> = Rc::new(
        choices
            .iter()
            .map(|c| {
                if c.options.is_empty() {
                    let check = gtk::CheckButton::with_label(&c.label);
                    check.set_active(c.initial == "true");
                    top.attach(&check, 1, row, 1, 1);
                    row += 1;
                    ChoiceControl::Check(c.id.clone(), check)
                } else {
                    let labels: Vec<&str> = c.options.iter().map(|(_, l)| l.as_str()).collect();
                    let dd = gtk::DropDown::from_strings(&labels);
                    let ids: Vec<String> = c.options.iter().map(|(id, _)| id.clone()).collect();
                    if let Some(i) = ids.iter().position(|id| *id == c.initial) {
                        dd.set_selected(u32::try_from(i).unwrap_or(0));
                    }
                    // (The app's label, not a mnemonic: it may hold a _.)
                    let l = gtk::Label::builder()
                        .label(format!("{}:", c.label))
                        .xalign(1.0)
                        .build();
                    dd.update_property(&[gtk::accessible::Property::Label(&c.label)]);
                    dd.set_hexpand(true);
                    top.attach(&l, 0, row, 1, 1);
                    top.attach(&dd, 1, row, 1, 1);
                    row += 1;
                    ChoiceControl::List(c.id.clone(), dd, ids)
                }
            })
            .collect(),
    );

    let location_entry = gtk::Entry::builder().placeholder_text("Path").build();
    field(
        &top,
        &mut row,
        "_Location:",
        location_entry.upcast_ref(),
        "Location",
    );
    location_entry.update_property(&[gtk::accessible::Property::Description(
        "Up and Down go through recent folders",
    )]);

    let more_btn = gtk::ToggleButton::with_mnemonic("_More options");
    more_btn.set_halign(gtk::Align::Start);
    more_btn.update_property(&[gtk::accessible::Property::Label("More options")]);
    top.attach(&more_btn, 1, row, 1, 1);

    // -- More options: places, Back/Forward/Up and the files --
    let back_btn = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Go back (Alt+Left)")
        .sensitive(false)
        .build();
    back_btn.update_property(&[gtk::accessible::Property::Label("Go back")]);

    let forward_btn = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Go forward (Alt+Right)")
        .sensitive(false)
        .build();
    forward_btn.update_property(&[gtk::accessible::Property::Label("Go forward")]);

    let up_btn = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Go to parent directory (Alt+Up)")
        .build();
    up_btn.update_property(&[gtk::accessible::Property::Label("Go to parent directory")]);

    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_box.add_css_class("linked");
    nav_box.append(&back_btn);
    nav_box.append(&forward_btn);
    nav_box.append(&up_btn);
    nav_box.set_margin_bottom(4);

    // Alt+Left / Alt+Right / Alt+Up, as in the main window — from anywhere,
    // More options open or not.
    {
        let nav_keys = gtk::ShortcutController::new();
        // Local: the controller sits on the dialog itself, which has no
        // ancestor to manage a Managed/Global one.
        nav_keys.set_scope(gtk::ShortcutScope::Local);
        for (accel, button) in [
            ("<Alt>Left", &back_btn),
            ("<Alt>Right", &forward_btn),
            ("<Alt>Up", &up_btn),
        ] {
            let button = button.clone();
            nav_keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(accel),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if button.is_sensitive() {
                        button.emit_clicked();
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        dlg.add_controller(nav_keys);
    }

    // View stack
    let view_stack = gtk::Stack::new();
    view_stack.set_transition_type(gtk::StackTransitionType::None);
    view_stack.add_named(grid_view.widget(), Some("grid"));
    view_stack.add_named(list_view.widget(), Some("list"));
    view_stack.set_visible_child_name("list");
    view_stack.set_vexpand(true);

    // Sidebar. Tab moves on from its list, on GTK versions that allow it
    // (4.18+; this is built for 4.14, so it's set by name).
    let sidebar = WayfinderSidebar::new();
    if gtk::check_version(4, 18, 0).is_none() {
        let leave = glib::Type::from_name("GtkListTabBehavior")
            .and_then(glib::EnumClass::with_type)
            .and_then(|class| class.to_value_by_nick("item"));
        if let Some(leave) = leave {
            sidebar
                .places_list()
                .set_property_from_value("tab-behavior", &leave);
        }
    }

    // Status label
    let status_label = gtk::Label::builder().xalign(0.0).margin_start(8).build();

    let files_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    files_box.append(&nav_box);
    files_box.append(&view_stack);
    files_box.append(&status_label);

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(sidebar.widget())
        .end_child(&files_box)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(180)
        .vexpand(true)
        .margin_top(8)
        .build();

    // -- Cancel and Save/Open --
    let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom_bar.set_margin_top(8);
    bottom_bar.set_margin_bottom(8);
    bottom_bar.set_margin_start(8);
    bottom_bar.set_margin_end(8);
    bottom_bar.set_halign(gtk::Align::End);

    let cancel_btn = gtk::Button::with_label("Cancel");
    cancel_btn.update_property(&[gtk::accessible::Property::Label("Cancel")]);

    // (The app's label may mark a mnemonic, "_Save": used as one, and not
    // read out as "underscore".)
    let accept_btn = gtk::Button::with_mnemonic(&accept_label);
    accept_btn.add_css_class("suggested-action");
    accept_btn.update_property(&[gtk::accessible::Property::Label(&without_mnemonic(
        &accept_label,
    ))]);

    bottom_bar.append(&cancel_btn);
    bottom_bar.append(&accept_btn);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&top);
    root.append(&paned);
    root.append(&bottom_bar);
    dlg.set_child(Some(&root));

    // -- List or grid: as Wayfinder was last left (the same setting) --
    let grid_shown = Rc::new(Cell::new(false));
    // To the files, list or grid, at the file selected if any.
    let focus_files: Rc<dyn Fn()> = {
        let (lv, gv, sel, grid) = (
            list_view.clone(),
            grid_view.clone(),
            selection.clone(),
            grid_shown.clone(),
        );
        Rc::new(move || {
            let pos = sel.selected();
            match (grid.get(), pos != gtk::INVALID_LIST_POSITION) {
                // (Plain focus in the grid: scrolling it with FOCUS after a
                // folder change upsets its focus tracking.)
                (true, _) => gv.grab_focus(),
                (false, true) => lv.grab_focus_at_selected(pos),
                (false, false) => lv.grab_focus(),
            }
        })
    };
    // Only the view shown has the files: a hidden one holding them too
    // confuses Orca.
    let show_view: Rc<dyn Fn(bool)> = {
        let (lv, gv, sel, stack, grid) = (
            list_view.clone(),
            grid_view.clone(),
            selection.clone(),
            view_stack.clone(),
            grid_shown.clone(),
        );
        Rc::new(move |as_grid: bool| {
            grid.set(as_grid);
            if as_grid {
                lv.column_view().set_model(gtk::SelectionModel::NONE);
                gv.set_model(&sel);
                stack.set_visible_child_name("grid");
            } else {
                gv.grid_view().set_model(gtk::SelectionModel::NONE);
                lv.set_model(&sel);
                stack.set_visible_child_name("list");
            }
        })
    };
    show_view(wayfinder::state::load_view_mode().as_deref() == Some("grid"));
    // Ctrl+1 grid, Ctrl+2 list, as in Wayfinder — from anywhere.
    {
        let keys = gtk::ShortcutController::new();
        keys.set_scope(gtk::ShortcutScope::Local);
        for (accel, as_grid) in [("<Ctrl>1", true), ("<Ctrl>2", false)] {
            let (show, focus, grid, d) = (
                show_view.clone(),
                focus_files.clone(),
                grid_shown.clone(),
                dlg.clone(),
            );
            let paned = paned.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(accel),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if grid.get() != as_grid {
                        show(as_grid);
                        wayfinder::state::save_view_mode(if as_grid { "grid" } else { "list" });
                    }
                    d.announce(
                        if as_grid { "Grid view" } else { "List view" },
                        AccessibleAnnouncementPriority::Medium,
                    );
                    if paned.is_visible() {
                        focus();
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        dlg.add_controller(keys);
    }

    // More options: open as it was left last time.
    paned.set_visible(memory.expanded);
    more_btn.set_active(memory.expanded);
    more_btn.update_state(&[gtk::accessible::State::Expanded(Some(memory.expanded))]);
    {
        let paned = paned.clone();
        more_btn.connect_toggled(move |b| {
            let open = b.is_active();
            paned.set_visible(open);
            b.update_state(&[gtk::accessible::State::Expanded(Some(open))]);
            Memory::update(|m| m.expanded = open);
        });
    }

    // A cached folder's background check changed it, or found it can no
    // longer be listed (the list has been emptied): say so, as the main
    // window does.
    // Weak references: the model holds these callbacks, so strong ones
    // would keep the model and dialog alive after the dialog closes.
    {
        let sel = selection.downgrade();
        let sl = status_label.downgrade();
        model.set_on_background_update(move || {
            if let (Some(sel), Some(sl)) = (sel.upgrade(), sl.upgrade()) {
                sl.set_text(&format!("{} items", sel.n_items()));
            }
        });
        let sl = status_label.downgrade();
        let window = dlg.downgrade();
        model.set_on_background_error(move |location, e| {
            log::error!("Failed to refresh {location}: {e}");
            let (Some(sl), Some(window)) = (sl.upgrade(), window.upgrade()) else {
                return;
            };
            sl.set_text("");
            window.announce(
                &format!(
                    "Error: could not open {}",
                    wayfinder::file_model::display_name(&location)
                ),
                AccessibleAnnouncementPriority::High,
            );
        });
    }

    // Wire ColumnView sorter. Sorting here isn't Wayfinder's own sort:
    // don't save it over that.
    list_view.set_persist_sort(false);
    let columns = list_view.column_view().columns();
    if let Some(first_col) = columns.item(0).and_downcast::<gtk::ColumnViewColumn>() {
        list_view
            .column_view()
            .sort_by_column(Some(&first_col), gtk::SortType::Ascending);
    }
    if let Some(cv_sorter) = list_view.column_view().sorter() {
        model.set_sorter(Some(&cv_sorter));
    }

    // Files chosen with Space (several only if the app allows), and what
    // that put in the name field: a name typed over it is a name typed.
    let chosen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let auto_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    // The name the user gave (to begin with, the app's): what the field
    // goes back to when the files chosen are let go.
    let typed_name: Rc<RefCell<String>> = Rc::new(RefCell::new(if is_save {
        suggested_name.to_string()
    } else {
        String::new()
    }));

    // The app's name, with the extension of the type now chosen.
    let reset_target: Rc<dyn Fn() -> String> = {
        let (filters, active, suggested) = (
            filters.clone(),
            active_filter.clone(),
            suggested_name.clone(),
        );
        Rc::new(move || {
            let now = active.get();
            if now.is_none() || now == Some(first_filter) {
                return suggested.to_string();
            }
            now.and_then(|i| filters.get(i))
                .and_then(|to| options::swap_extension(&suggested, filters.get(first_filter), to))
                .unwrap_or_else(|| suggested.to_string())
        })
    };
    let update_reset: Rc<dyn Fn()> = {
        let (btn, entry, target) = (reset_btn.clone(), name_entry.clone(), reset_target.clone());
        Rc::new(move || btn.set_visible(is_save && entry.text() != target()))
    };
    {
        // A name not put there by choosing files is the user's.
        let (auto_text, typed, update) =
            (auto_text.clone(), typed_name.clone(), update_reset.clone());
        name_entry.connect_changed(move |entry| {
            let text = entry.text();
            if text != *auto_text.borrow() {
                *typed.borrow_mut() = text.to_string();
            }
            update();
        });
    }
    {
        let (chosen, auto_text, entry) = (chosen.clone(), auto_text.clone(), name_entry.clone());
        let (target, d) = (reset_target.clone(), dlg.clone());
        reset_btn.connect_clicked(move |_| {
            chosen.borrow_mut().clear();
            auto_text.borrow_mut().clear();
            let name = target();
            entry.set_text(&name);
            // (The button has gone: back to the name.)
            entry.grab_focus();
            entry.select_region(0, options::stem_chars(&name));
            d.announce(
                &format!("File name reset to {name}"),
                AccessibleAnnouncementPriority::Medium,
            );
        });
    }

    // -- Going places --
    // Show a folder (history already updated)...
    let show_folder: Rc<dyn Fn(&str)> = {
        let model = model.clone();
        let le = location_entry.clone();
        let sl = status_label.clone();
        let nav = nav.clone();
        let bb = back_btn.clone();
        let fb = forward_btn.clone();
        let ub = up_btn.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        Rc::new(move |path: &str| load_dir(&model, path, &le, &sl, &nav, &bb, &fb, &ub, &sel, &d))
    };
    // ...or go to one.
    let go_to: Rc<dyn Fn(&str)> = {
        let nav = nav.clone();
        let show = show_folder.clone();
        Rc::new(move |path: &str| {
            nav.borrow_mut().navigate_to(PathBuf::from(path));
            show(path);
        })
    };

    // Keys in the files, list or grid: Tab on and back, Ctrl+A, type-ahead;
    // in the list, Left/Right read its columns (in the grid they move).
    let col_names: &[&str] = &["Name", "Size", "Date Modified", "Kind"];
    let current_column: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let view_keys = |grid: bool| {
        let col_controller = gtk::EventControllerKey::new();
        col_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cc = current_column.clone();
        let sel = selection.clone();
        let d = dlg.clone();
        let cancel = cancel_btn.clone();
        let (bb, fb, ub) = (back_btn.clone(), forward_btn.clone(), up_btn.clone());
        let places = sidebar.places_list().clone();
        let (chosen, auto_text, entry) = (chosen.clone(), auto_text.clone(), name_entry.clone());
        let typed = typed_name.clone();
        // Type-ahead: the letters so far, and when the last came.
        let ahead: Rc<RefCell<(String, Option<std::time::Instant>)>> =
            Rc::new(RefCell::new((String::new(), None)));
        let cv = list_view.column_view().clone();
        let gv = grid_view.grid_view().clone();
        col_controller.connect_key_pressed(move |_ctrl, key, _code, mods| {
            use gtk::gdk;
            let back = key == gdk::Key::ISO_Left_Tab
                || (key == gdk::Key::Tab && mods.contains(gdk::ModifierType::SHIFT_MASK));
            if back {
                // Back: Up, Forward, Back (those that can be pressed), then
                // the places.
                match [&ub, &fb, &bb].into_iter().find(|b| b.is_sensitive()) {
                    Some(b) => b.grab_focus(),
                    None => places.grab_focus(),
                };
                glib::Propagation::Stop
            } else if key == gdk::Key::Tab {
                // On: Cancel, then Save/Open — rather than round the files.
                cancel.grab_focus();
                glib::Propagation::Stop
            } else if multiple
                && key == gdk::Key::a
                && mods.contains(gdk::ModifierType::CONTROL_MASK)
            {
                // Every file shown, chosen.
                let files: Vec<FileObject> = (0..sel.n_items())
                    .filter_map(|i| sel.item(i).and_downcast::<FileObject>())
                    .filter(|f| !f.is_directory())
                    .collect();
                *chosen.borrow_mut() = files.iter().map(|f| f.path()).collect();
                let back_to = typed.borrow().clone();
                show_chosen(&chosen.borrow(), &entry, &auto_text, &back_to);
                d.announce(
                    &format!("Selected {} files", files.len()),
                    AccessibleAnnouncementPriority::Medium,
                );
                glib::Propagation::Stop
            } else if let Some(c) = key
                .to_unicode()
                .filter(|c| !c.is_control() && *c != ' ')
                .filter(|_| {
                    !mods.intersects(
                        gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::ALT_MASK
                            | gdk::ModifierType::SUPER_MASK,
                    )
                })
            {
                // Letters typed: to the name starting with them. A second's
                // pause starts afresh.
                let mut ahead = ahead.borrow_mut();
                let now = std::time::Instant::now();
                if ahead
                    .1
                    .is_none_or(|t| now.duration_since(t) > TYPE_AHEAD_PAUSE)
                {
                    ahead.0.clear();
                }
                ahead.0.push(c);
                ahead.1 = Some(now);
                let names: Vec<String> = (0..sel.n_items())
                    .filter_map(|i| sel.item(i).and_downcast::<FileObject>())
                    .map(|f| f.name())
                    .collect();
                let current = (sel.selected() != gtk::INVALID_LIST_POSITION)
                    .then(|| usize::try_from(sel.selected()).ok())
                    .flatten();
                match options::type_ahead(&names, current, &ahead.0)
                    .and_then(|i| u32::try_from(i).ok())
                {
                    Some(i) if Some(i as usize) == current => {}
                    Some(i) => {
                        let flags = gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT;
                        if grid {
                            gv.scroll_to(i, flags, None);
                        } else {
                            cv.scroll_to(i, None::<&gtk::ColumnViewColumn>, flags, None);
                        }
                    }
                    None => d.announce(
                        &format!("Nothing starting with {}", ahead.0),
                        AccessibleAnnouncementPriority::Medium,
                    ),
                }
                glib::Propagation::Stop
            } else if !grid
                && (key == gdk::Key::Left || key == gdk::Key::Right)
                && !mods.contains(gdk::ModifierType::ALT_MASK)
            {
                let col = cc.get();
                let next = if key == gdk::Key::Left {
                    col.checked_sub(1)
                } else {
                    (col + 1 < col_names.len()).then_some(col + 1)
                };
                if let Some(next) = next {
                    cc.set(next);
                    if let Some(file) = sel.selected_item().and_downcast::<FileObject>() {
                        d.announce(
                            &format!("{}: {}", col_names[next], column_value(&file, next)),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        col_controller
    };
    list_view.column_view().add_controller(view_keys(false));
    grid_view.grid_view().add_controller(view_keys(true));

    // Announce column value on selection change in list view — and whether
    // it's one chosen.
    {
        let cc = current_column.clone();
        let d = dlg.clone();
        let chosen = chosen.clone();
        let grid = grid_shown.clone();
        selection.connect_selected_item_notify(move |sel| {
            // (The grid has no columns: its name.)
            let col = if grid.get() { 0 } else { cc.get() };
            if let Some(file) = sel.selected_item().and_downcast::<FileObject>() {
                let mark = if chosen.borrow().contains(&file.path()) {
                    ", selected"
                } else {
                    ""
                };
                d.announce(
                    &format!("{}: {}{mark}", col_names[col], column_value(&file, col)),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
        });
    }

    // -- Load initial directory --
    let initial_path = nav.borrow().current().to_string_lossy().to_string();
    show_folder(&initial_path);

    // -- Navigation button handlers --
    {
        let nav = nav.clone();
        let show = show_folder.clone();
        back_btn.connect_clicked(move |_| {
            // Bound first, so the borrow ends before load_dir borrows again.
            let back = nav.borrow_mut().go_back().cloned();
            if let Some(path) = back {
                show(&path.to_string_lossy());
            }
        });
    }
    {
        let nav = nav.clone();
        let show = show_folder.clone();
        forward_btn.connect_clicked(move |_| {
            let forward = nav.borrow_mut().go_forward().cloned();
            if let Some(path) = forward {
                show(&path.to_string_lossy());
            }
        });
    }
    {
        let nav = nav.clone();
        let go = go_to.clone();
        up_btn.connect_clicked(move |_| {
            let up = nav.borrow().go_up();
            if let Some(parent) = up {
                go(&parent.to_string_lossy());
            }
        });
    }

    // -- Sidebar navigation --
    {
        let go = go_to.clone();
        let d = dlg.clone();
        let focus = focus_files.clone();
        sidebar.connect_place_activated(move |path| {
            // The Bin and Recent Files aren't folders to choose from.
            if path.starts_with("trash:") || path.starts_with("recent:") {
                d.announce(
                    &format!(
                        "{} can't be opened in the file chooser",
                        wayfinder::file_model::display_name(&path)
                    ),
                    AccessibleAnnouncementPriority::Medium,
                );
                return;
            }
            go(&path);
            // To its files, as in the main window.
            focus();
        });
    }

    // A choice belongs to the folder it was made in: moving elsewhere
    // (every move shows the new location in the entry) clears it — and
    // the names it put in the name field, unless that's a name to save as.
    {
        let (chosen, auto_text, entry) = (chosen.clone(), auto_text.clone(), name_entry.clone());
        let typed = typed_name.clone();
        location_entry.connect_changed(move |_| {
            if chosen.borrow_mut().drain(..).next().is_some() && !is_save {
                let was_chosen = entry.text() == *auto_text.borrow();
                auto_text.borrow_mut().clear();
                if was_chosen {
                    let back_to = typed.borrow().clone();
                    entry.set_text(&back_to);
                }
            }
        });
    }

    // -- Enter in the location: go there --
    // (Up/Down set `stay`: going through recent folders keeps the focus in
    // the location.)
    let stay = Rc::new(Cell::new(false));
    {
        let model = model.clone();
        let d = dlg.clone();
        let focus = focus_files.clone();
        let go = go_to.clone();
        let paned = paned.clone();
        let stay = stay.clone();
        // Enters, counted: only the latest one's check goes anywhere (one
        // for a corrected path mustn't lose to the one before it).
        let enters = Rc::new(Cell::new(0u64));
        location_entry.connect_activate(move |entry| {
            let stay = stay.replace(false);
            let enter = enters.get() + 1;
            enters.set(enter);
            let enters = enters.clone();
            let path = wayfinder::path_complete::expand_tilde(entry.text().trim());
            if !path.starts_with('/') {
                d.announce(
                    "Type a full path, starting with / or ~",
                    AccessibleAnnouncementPriority::Medium,
                );
                return;
            }
            // `/a/b/` and `/a/b/..` as plain paths, as in the main window.
            let path = wayfinder::path_complete::clean(&path);
            // Only a folder can be gone to (or saved in); checking may
            // touch a slow mount, so not on the main thread.
            let check = path.clone();
            let generation = model.generation();
            let (model, d, focus, go, paned) = (
                model.clone(),
                d.clone(),
                focus.clone(),
                go.clone(),
                paned.clone(),
            );
            let waiting = wayfinder::background::waiting_message(&check);
            let d_slow = d.clone();
            wayfinder::background::run(
                move || std::path::Path::new(&check).is_dir(),
                move |is_dir| {
                    // Gone somewhere else while it was checked, or another
                    // Enter since: leave it.
                    if model.generation() != generation || enters.get() != enter {
                        return;
                    }
                    if !is_dir {
                        d.announce(
                            &format!("Not a folder: {path}"),
                            AccessibleAnnouncementPriority::High,
                        );
                        return;
                    }
                    go(&path);
                    // To its files, if they're shown.
                    if !stay && paned.is_visible() {
                        focus();
                    }
                },
                move || {
                    if let Some(msg) = waiting {
                        d_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                    }
                },
            );
        });
    }

    // -- Up/Down in the location: recent folders, gone to at once --
    {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let entry = location_entry.clone();
        let d = dlg.clone();
        let stay = stay.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            use gtk::gdk;
            let down = match key {
                gdk::Key::Down | gdk::Key::KP_Down => true,
                gdk::Key::Up | gdk::Key::KP_Up => false,
                _ => return glib::Propagation::Proceed,
            };
            if mods.intersects(
                gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::SHIFT_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            let recent = Memory::load().recent;
            if recent.is_empty() {
                d.announce(
                    "No recent folders yet",
                    AccessibleAnnouncementPriority::Medium,
                );
                return glib::Propagation::Stop;
            }
            match options::recent_step(&recent, &entry.text(), down) {
                Some(i) => {
                    entry.set_text(&recent[i]);
                    entry.set_position(-1);
                    stay.set(true);
                    entry.emit_activate();
                }
                None => d.announce(
                    if down {
                        "Last recent folder"
                    } else {
                        "First recent folder"
                    },
                    AccessibleAnnouncementPriority::Medium,
                ),
            }
            glib::Propagation::Stop
        });
        location_entry.add_controller(keys);
    }

    // -- Space to choose a file (deliberate selection); again to unchoose --
    let space_on = |view: &gtk::Widget| {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let sel = selection.clone();
        let chosen = chosen.clone();
        let entry = name_entry.clone();
        let auto_text = auto_text.clone();
        let typed = typed_name.clone();
        let d = dlg.clone();
        keys.connect_key_pressed(move |_ctrl, key, _code, _mods| {
            if key != gtk::gdk::Key::space {
                return glib::Propagation::Proceed;
            }
            if let Some(file) = sel.selected_item().and_downcast::<FileObject>() {
                space_choose(
                    &file,
                    directory_mode,
                    multiple,
                    &chosen,
                    &entry,
                    &auto_text,
                    &typed.borrow().clone(),
                    &d,
                );
            }
            glib::Propagation::Stop
        });
        view.add_controller(keys);
    };
    space_on(list_view.column_view().upcast_ref());
    space_on(grid_view.grid_view().upcast_ref());

    // -- Enter opens folders, doesn't select files --
    for view in [
        list_view.column_view().upcast_ref::<gtk::Widget>(),
        grid_view.grid_view().upcast_ref(),
    ] {
        let enter_controller = gtk::EventControllerKey::new();
        enter_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let sel = selection.clone();
        let go = go_to.clone();
        enter_controller.connect_key_pressed(move |_ctrl, key, _code, _mods| {
            if key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter {
                if let Some(file) = sel.selected_item().and_downcast::<FileObject>() {
                    if file.is_directory() {
                        go(&file.path());
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        view.add_controller(enter_controller);
    }

    // -- The file type: filters the list; when saving, the name follows --
    if let Some(dd) = type_dropdown.as_ref() {
        let active = active_filter.clone();
        let filters = filters.clone();
        let type_filter = type_filter.clone();
        let sel = selection.clone();
        let sl = status_label.clone();
        let entry = name_entry.clone();
        let d = dlg.clone();
        let update = update_reset.clone();
        dd.connect_selected_notify(move |dd| {
            let Ok(i) = usize::try_from(dd.selected()) else {
                return;
            };
            if i >= filters.len() {
                return;
            }
            let before = active.replace(Some(i));
            type_filter.changed(gtk::FilterChange::Different);
            sl.set_text(&format!("{} items", sel.n_items()));
            if is_save {
                let old = before.and_then(|b| filters.get(b));
                if let Some(name) = options::swap_extension(&entry.text(), old, &filters[i]) {
                    entry.set_text(&name);
                    d.announce(
                        &format!("File name {name}"),
                        AccessibleAnnouncementPriority::Low,
                    );
                }
            }
            update();
        });
    }

    // What goes back with the files: the app's choices and the type.
    let extras = {
        let controls = choice_controls.clone();
        let active = active_filter.clone();
        let filters = filters.clone();
        move || -> (Vec<(String, String)>, Option<Filter>) {
            let values = controls
                .iter()
                .map(|c| match c {
                    ChoiceControl::Check(id, b) => (id.clone(), b.is_active().to_string()),
                    ChoiceControl::List(id, dd, ids) => (
                        id.clone(),
                        usize::try_from(dd.selected())
                            .ok()
                            .and_then(|i| ids.get(i).cloned())
                            .unwrap_or_default(),
                    ),
                })
                .collect();
            (values, active.get().and_then(|i| filters.get(i).cloned()))
        }
    };

    // -- Cancel --
    let tx_cancel = result_tx.clone();
    let d_cancel = dlg.clone();
    cancel_btn.connect_clicked(move |_| {
        if let Some(tx) = tx_cancel.borrow_mut().take() {
            let _ = tx.send(ChooserResult {
                uris: vec![],
                cancelled: true,
                choices: vec![],
                current_filter: None,
            });
        }
        d_cancel.close();
    });

    // -- Accept --
    let tx_accept = result_tx.clone();
    let chosen_accept = chosen.clone();
    let auto_accept = auto_text.clone();
    let typed_accept = typed_name.clone();
    let fn_accept = name_entry.clone();
    let model_accept = model.clone();
    let d_accept = dlg.clone();
    let le_accept = location_entry.clone();
    let go_accept = go_to.clone();
    let paned_accept = paned.clone();
    let focus_accept = focus_files.clone();
    // Save presses, counted: a Replace check under way acts only if it's
    // the latest press's, the dialog is still open, and it still shows the
    // folder and name it checked (otherwise a corrected name, another
    // folder or a cancel would lose to it).
    let checking_accept = Rc::new(Cell::new(0u64));
    accept_btn.connect_clicked(move |_| {
        let press = checking_accept.get() + 1;
        checking_accept.set(press);
        let current_dir = model_accept.current_path();
        // A location typed but not gone to (Enter) isn't where this would
        // save or what it would choose: say so, rather than use the folder
        // shown before.
        let typed = le_accept.text().to_string();
        if location_not_open(&typed, &current_dir) {
            d_accept.announce(
                &format!(
                    "{} isn't open yet: press Enter in the location to go there",
                    typed.trim()
                ),
                AccessibleAnnouncementPriority::High,
            );
            le_accept.grab_focus();
            return;
        }
        let send = {
            let tx = tx_accept.clone();
            let d = d_accept.clone();
            let extras = extras.clone();
            let folder = current_dir.clone();
            move |paths: &[String]| {
                if let Some(tx) = tx.borrow_mut().take() {
                    let (choices, current_filter) = extras();
                    let _ = tx.send(ChooserResult {
                        uris: paths
                            .iter()
                            .map(|p| gio::File::for_path(p).uri().to_string())
                            .collect(),
                        cancelled: false,
                        choices,
                        current_filter,
                    });
                    Memory::update(|m| m.used(&folder));
                }
                d.close();
            }
        };

        if is_save {
            // For Save: the filename entry text in the current directory.
            let name = fn_accept.text().to_string();
            if name.is_empty() {
                d_accept.announce("No file name", AccessibleAnnouncementPriority::High);
                return;
            }
            // A name, saved in the folder shown — not a path elsewhere (an
            // app may suggest one, and saving it would go there unseen).
            if name.contains('/') || name == "." || name == ".." {
                d_accept.announce(
                    "A file name can't contain / or be . or ..",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }
            let full_path = std::path::Path::new(&current_dir)
                .join(&name)
                .to_string_lossy()
                .to_string();
            // Ask before replacing an existing file, as GTK's chooser does.
            // (Checking may touch a slow mount: not on the main thread.)
            let check = full_path.clone();
            let d = d_accept.clone();
            let d_slow = d_accept.clone();
            let waiting = wayfinder::background::waiting_message(&check);
            // (Only the latest press's check acts: see checking_accept.)
            // Why this press's check may no longer act, if so: "" for a
            // newer press or the dialog gone (quietly: the newer press
            // speaks), or what changed since (the folder and name shown,
            // as written, or another location typed).
            let stale = Rc::new({
                let model = model_accept.clone();
                let name_entry = fn_accept.clone();
                let location = le_accept.clone();
                let d = d_accept.clone();
                let (checked_folder, checked_name) = (current_dir.clone(), name.clone());
                let checking = checking_accept.clone();
                move || -> Option<&'static str> {
                    if checking.get() != press || !d.is_visible() {
                        return Some("");
                    }
                    let folder = model.current_path();
                    if location_not_open(&location.text(), &folder) {
                        return Some("Not saved: another location was typed. Press Enter in it to go there, then Save.");
                    }
                    if folder != checked_folder || name_entry.text() != checked_name {
                        return Some("Not saved: the folder or name changed. Press Save again.");
                    }
                    None
                }
            });
            let stale_choice = stale.clone();
            let checking_slow = checking_accept.clone();
            let d_choice = d_accept.clone();
            // "Press Enter in it": focus goes there, as for a press.
            let say_stale = {
                let location = le_accept.clone();
                move |d: &gtk::Window, why: &str| {
                    d.announce(why, AccessibleAnnouncementPriority::Medium);
                    if why.contains("location") {
                        location.grab_focus();
                    }
                }
            };
            let say_stale_choice = say_stale.clone();
            let (model_slow, folder_slow) = (model_accept.clone(), current_dir.clone());
            wayfinder::background::run(
                // (The folder itself first: one that failed to open — gone,
                // say — isn't a place to save in.)
                move || {
                    let path = std::path::Path::new(&check);
                    match path.parent() {
                        Some(folder) if !folder.is_dir() => {
                            Err(format!("the folder {} can't be opened", folder.display()))
                        }
                        _ => save_target(path),
                    }
                },
                move |found| {
                    match stale() {
                        Some("") => return,
                        Some(why) => {
                            say_stale(&d, why);
                            return;
                        }
                        None => {}
                    }
                    let found = match found {
                        Ok(found) => found,
                        Err(e) => {
                            d.announce(
                                &format!("Couldn't tell whether {name} exists: {e}"),
                                AccessibleAnnouncementPriority::High,
                            );
                            return;
                        }
                    };
                    let Some(refused) = found else {
                        send(std::slice::from_ref(&full_path));
                        return;
                    };
                    // Not a file to save over: the app couldn't write to a
                    // folder, and through a broken link would write
                    // somewhere else.
                    if let Some(what) = refused {
                        d.announce(
                            &format!("{name} is {what}: give the file another name"),
                            AccessibleAnnouncementPriority::High,
                        );
                        return;
                    }
                    let confirm = gtk::AlertDialog::builder()
                        .message(format!("Replace “{name}”?"))
                        .detail("A file with this name already exists. Replacing it overwrites its contents.")
                        .buttons(["Cancel", "Replace"])
                        .cancel_button(0)
                        .default_button(0)
                        .build();
                    confirm.choose(Some(&d), gio::Cancellable::NONE, move |choice| {
                        if choice != Ok(1) {
                            return;
                        }
                        // Changed while the question was open: said, not
                        // saved somewhere else.
                        match stale_choice() {
                            None => send(std::slice::from_ref(&full_path)),
                            Some("") => {}
                            Some(why) => say_stale_choice(&d_choice, why),
                        }
                    });
                },
                move || {
                    // (Said even if the name is being edited meanwhile: the
                    // check is under way all the same — but not once another
                    // folder is shown, which it isn't about.)
                    if checking_slow.get() != press
                        || !d_slow.is_visible()
                        || model_slow.current_path() != folder_slow
                    {
                        return;
                    }
                    if let Some(msg) = waiting {
                        d_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                    }
                },
            );
            return;
        }

        // For Open: the files chosen with Space, if the name field still
        // shows them and they're in this folder (they may have been chosen
        // before moving elsewhere). Choosing a folder, with none chosen,
        // means the one being shown.
        let text = fn_accept.text().to_string();
        if text.trim().is_empty() || text == *auto_accept.borrow() {
            let here: Vec<String> = chosen_accept
                .borrow()
                .iter()
                .filter(|p| {
                    std::path::Path::new(p).parent() == Some(std::path::Path::new(&current_dir))
                })
                .cloned()
                .collect();
            if !here.is_empty() {
                send(&here);
            } else if directory_mode {
                send(std::slice::from_ref(&current_dir));
            } else {
                d_accept.announce("No file selected", AccessibleAnnouncementPriority::High);
            }
            return;
        }

        // Names typed: in this folder, or a full path. A folder typed is
        // gone to (unless folders are what's being chosen).
        let names = options::typed_names(&text);
        if names.len() > 1 && !multiple {
            d_accept.announce(
                "Only one can be chosen here",
                AccessibleAnnouncementPriority::High,
            );
            return;
        }
        let paths: Vec<String> = names.iter().map(|n| resolve_typed(n, &current_dir)).collect();
        let check = paths.clone();
        let generation = model_accept.generation();
        let (model, d, entry, go, paned, focus, checking) = (
            model_accept.clone(),
            d_accept.clone(),
            fn_accept.clone(),
            go_accept.clone(),
            paned_accept.clone(),
            focus_accept.clone(),
            checking_accept.clone(),
        );
        let typed = typed_accept.clone();
        let waiting = paths
            .first()
            .and_then(|p| wayfinder::background::waiting_message(p));
        let d_slow = d_accept.clone();
        wayfinder::background::run(
            // Each: Some(is a folder), or None if it isn't there.
            move || {
                check
                    .iter()
                    .map(|p| std::fs::metadata(p).ok().map(|m| m.is_dir()))
                    .collect::<Vec<_>>()
            },
            move |kinds| {
                // Pressed again, gone elsewhere or retyped since: leave it.
                if checking.get() != press
                    || !d.is_visible()
                    || model.generation() != generation
                    || entry.text() != text
                {
                    return;
                }
                for (name, kind) in names.iter().zip(&kinds) {
                    let problem = match kind {
                        None => Some(format!("{name} doesn't exist")),
                        Some(false) if directory_mode => Some(format!("{name} isn't a folder")),
                        Some(true) if !directory_mode && names.len() > 1 => {
                            Some(format!("{name} is a folder"))
                        }
                        _ => None,
                    };
                    if let Some(problem) = problem {
                        d.announce(&problem, AccessibleAnnouncementPriority::High);
                        return;
                    }
                }
                if !directory_mode && kinds == [Some(true)] {
                    // (Cleared: a folder gone to isn't a name to keep.)
                    entry.set_text("");
                    typed.borrow_mut().clear();
                    go(&paths[0]);
                    if paned.is_visible() {
                        focus();
                    }
                    return;
                }
                send(&paths);
            },
            move || {
                if let Some(msg) = waiting {
                    d_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
                }
            },
        );
    });

    // Enter in the file name field accepts; Escape cancels.
    {
        let accept = accept_btn.clone();
        name_entry.connect_activate(move |_| accept.emit_clicked());
    }
    {
        let cancel = cancel_btn.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cancel.emit_clicked();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        dlg.add_controller(keys);
    }

    // Close window = cancel
    let tx_close = result_tx.clone();
    let m = model.clone();
    dlg.connect_close_request(move |_| {
        // Don't keep polling a folder still opening on a stuck mount.
        m.abandon_loading();
        if let Some(tx) = tx_close.borrow_mut().take() {
            let _ = tx.send(ChooserResult {
                uris: vec![],
                cancelled: true,
                choices: vec![],
                current_filter: None,
            });
        }
        glib::Propagation::Proceed
    });

    dlg.present();
    // The cursor starts in the file name, its name part selected: typing
    // replaces the name and keeps the type.
    name_entry.grab_focus();
    let stem = options::stem_chars(&name_entry.text());
    name_entry.select_region(0, stem);
}

/// Letters typed in the file list further apart than this start a new
/// search.
const TYPE_AHEAD_PAUSE: std::time::Duration = std::time::Duration::from_millis(1000);

/// What column `col` of the list says for `file`.
fn column_value(file: &FileObject, col: usize) -> String {
    match col {
        0 => file.name(),
        1 => file.size_display(),
        2 => file.modified_display(),
        3 => file.file_type_name(),
        _ => String::new(),
    }
}

/// A name typed in the name field, as a full path: one starting with / or
/// ~ as it is; anything else in the folder shown.
fn resolve_typed(name: &str, current_dir: &str) -> String {
    use wayfinder::path_complete::{clean, expand_tilde};
    let expanded = expand_tilde(name);
    if expanded.starts_with('/') {
        clean(&expanded)
    } else {
        clean(
            &std::path::Path::new(current_dir)
                .join(name)
                .to_string_lossy(),
        )
    }
}

/// The name field showing the files chosen, noted as what the chooser put
/// there; or, with none, `empty` (the user's own name).
fn show_chosen(chosen: &[String], entry: &gtk::Entry, auto_text: &RefCell<String>, empty: &str) {
    let names: Vec<String> = chosen
        .iter()
        .filter_map(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();
    if names.is_empty() {
        // Back to the user's own name: theirs, not the chooser's.
        auto_text.borrow_mut().clear();
        entry.set_text(empty);
    } else {
        // (Noted first: setting the text tells whoever's watching.)
        let text = options::names_text(&names);
        *auto_text.borrow_mut() = text.clone();
        entry.set_text(&text);
    }
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
    // Show where we're going straight away, so Back works while a slow
    // mount is still answering.
    location_entry.set_text(path);
    back_btn.set_sensitive(nav.borrow().can_go_back());
    forward_btn.set_sensitive(nav.borrow().can_go_forward());
    up_btn.set_sensitive(path != "/");

    let dir_name = wayfinder::file_model::display_name(path);
    {
        let status_label = status_label.clone();
        let selection = selection.clone();
        let window = window.clone();
        model.load_directory(path, false, move |result| match result {
            Ok(_) => {
                let count = selection.n_items();
                status_label.set_text(&format!("{count} items"));
                selection.set_selected(gtk::INVALID_LIST_POSITION); // Clear selection
                window.announce(
                    &format!("Opened {dir_name}, {count} items"),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
            Err(e) => {
                status_label.set_text("");
                window.announce(&format!("Error: {e}"), AccessibleAnnouncementPriority::High);
            }
        });
    }

    // Still waiting after a moment: say so, and how to back out.
    let status_label = status_label.clone();
    let window = window.clone();
    model.notify_if_waiting(path, move |msg| {
        status_label.set_text(&msg);
        window.announce(&msg, AccessibleAnnouncementPriority::Medium);
    });
}

/// What's at `path`, where a file is to be saved: `Ok(None)` nothing (free);
/// `Ok(Some(None))` a file to replace (a link to one counts, as in GTK's
/// chooser); `Ok(Some(Some(what)))` something not to save over — a folder,
/// a pipe or device (the app would hang writing to it), a link leading
/// nowhere (the app would write wherever it pointed); `Err` if it can't be
/// told (not taken for free: an existing file would be written over
/// unasked).
fn save_target(path: &std::path::Path) -> Result<Option<Option<String>>, String> {
    let kind = |m: &std::fs::Metadata| {
        if m.is_dir() {
            Some("a folder".to_string())
        } else if m.is_file() {
            None
        } else {
            Some("not a regular file".to_string())
        }
    };
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(m) if !m.file_type().is_symlink() => Ok(Some(kind(&m))),
        Ok(_) => match std::fs::metadata(path) {
            Ok(m) => Ok(Some(kind(&m))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Some(Some("a link that leads nowhere".to_string())))
            }
            Err(e) => Ok(Some(Some(format!("a link that can't be followed ({e})")))),
        },
    }
}

/// A GTK label with its mnemonic marks taken out: `_Save` is "Save", `__`
/// an underscore.
fn without_mnemonic(label: &str) -> String {
    let mut out = String::new();
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether the location typed (`typed`) is somewhere other than the folder
/// shown (`current`): typed but not gone to. As written or trimmed (a
/// folder's name may end in a space; a stray one typed is no matter).
fn location_not_open(typed: &str, current: &str) -> bool {
    use wayfinder::path_complete::{clean, expand_tilde};
    if typed.trim().is_empty() {
        return false;
    }
    let current = clean(current);
    clean(&expand_tilde(typed)) != current && clean(&expand_tilde(typed.trim())) != current
}

/// Space on the focused item: choose it, or, if it is already chosen,
/// unchoose it (so a file picked to replace can be let go again). Several
/// may be chosen if `multiple`; otherwise choosing one replaces the last.
#[allow(clippy::too_many_arguments)]
fn space_choose(
    file: &FileObject,
    directory_mode: bool,
    multiple: bool,
    chosen: &RefCell<Vec<String>>,
    name_entry: &gtk::Entry,
    auto_text: &RefCell<String>,
    back_to: &str,
    dlg: &gtk::Window,
) {
    // Choosing folders: only a folder; files: only a file.
    if file.is_directory() != directory_mode {
        let hint = if directory_mode {
            "Not a folder: choose a folder"
        } else {
            "That's a folder: press Enter to open it, Space on a file to choose it"
        };
        dlg.announce(hint, AccessibleAnnouncementPriority::Medium);
        return;
    }
    let name = file.name();
    let added = toggle_chosen(&mut chosen.borrow_mut(), &file.path(), multiple);
    show_chosen(&chosen.borrow(), name_entry, auto_text, back_to);
    let count = chosen.borrow().len();
    let more = if multiple && count > 1 {
        format!(", {count} selected")
    } else {
        String::new()
    };
    let what = if added { "Selected" } else { "Deselected" };
    dlg.announce(
        &format!("{what} {name}{more}"),
        AccessibleAnnouncementPriority::Medium,
    );
}

/// Space on `path`: out if it's chosen, else in (alongside the others only
/// if `multiple`). Whether it went in.
fn toggle_chosen(chosen: &mut Vec<String>, path: &str, multiple: bool) -> bool {
    if let Some(i) = chosen.iter().position(|p| p == path) {
        chosen.remove(i);
        return false;
    }
    if !multiple {
        chosen.clear();
    }
    chosen.push(path.to_string());
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn space_on_the_chosen_file_unchooses_it() {
        use super::toggle_chosen;
        let mut c = Vec::new();
        assert!(toggle_chosen(&mut c, "/a/b", false));
        assert!(toggle_chosen(&mut c, "/a/c", false));
        assert_eq!(c, vec!["/a/c"]);
        assert!(!toggle_chosen(&mut c, "/a/c", false));
        assert!(c.is_empty());
        // Several: each in and out on its own.
        toggle_chosen(&mut c, "/a/b", true);
        toggle_chosen(&mut c, "/a/c", true);
        assert!(!toggle_chosen(&mut c, "/a/b", true));
        assert_eq!(c, vec!["/a/c"]);
    }

    #[test]
    fn typed_names_resolve_in_the_folder_shown() {
        use super::resolve_typed;
        assert_eq!(resolve_typed("a.txt", "/home/u"), "/home/u/a.txt");
        assert_eq!(resolve_typed("../b", "/home/u"), "/home/b");
        assert_eq!(resolve_typed("/etc/x", "/home/u"), "/etc/x");
    }

    use super::{location_not_open, save_target, without_mnemonic};

    #[test]
    fn mnemonic_marks_arent_read_out() {
        assert_eq!(without_mnemonic("_Save"), "Save");
        assert_eq!(without_mnemonic("Save __As"), "Save _As");
        assert_eq!(without_mnemonic("Open"), "Open");
    }

    #[test]
    fn what_a_save_would_go_over() {
        let d = std::env::temp_dir().join(format!("wayfinder-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dir")).unwrap();
        std::fs::write(d.join("file"), "").unwrap();
        std::os::unix::fs::symlink(d.join("file"), d.join("to-file")).unwrap();
        std::os::unix::fs::symlink(d.join("missing"), d.join("nowhere")).unwrap();
        let free = save_target(&d.join("new"));
        let file = save_target(&d.join("file"));
        let to_file = save_target(&d.join("to-file"));
        let dir = save_target(&d.join("dir"));
        let nowhere = save_target(&d.join("nowhere"));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(free, Ok(None));
        assert_eq!(file, Ok(Some(None)));
        assert_eq!(to_file, Ok(Some(None)));
        assert_eq!(dir, Ok(Some(Some("a folder".to_string()))));
        assert_eq!(
            nowhere,
            Ok(Some(Some("a link that leads nowhere".to_string())))
        );
    }

    #[test]
    fn a_typed_location_counts_only_if_its_somewhere_else() {
        assert!(!location_not_open("", "/home/u"));
        assert!(!location_not_open("/home/u/", "/home/u"));
        assert!(!location_not_open("/home/u ", "/home/u"));
        assert!(!location_not_open("/home/u/Project ", "/home/u/Project "));
        assert!(location_not_open("/home/u/Projects", "/home/u"));
    }
}
