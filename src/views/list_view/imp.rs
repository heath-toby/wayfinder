use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{ColumnView, ColumnViewColumn, CustomSorter, ScrolledWindow, SignalListItemFactory};

use crate::file_object::FileObject;

pub struct ListViewInner {
    pub column_view: ColumnView,
    pub scrolled_window: ScrolledWindow,
}

impl Default for ListViewInner {
    fn default() -> Self {
        let column_view = ColumnView::builder()
            .show_column_separators(true)
            .show_row_separators(true)
            .build();

        // Whether the sort is descending: the folders-first rule reads it
        // (see folders_first).
        let descending = std::rc::Rc::new(std::cell::Cell::new(!crate::state::load_sort_state().1));

        // Name column with sorter
        let name_factory = SignalListItemFactory::new();
        setup_name_factory(&name_factory);
        let desc = descending.clone();
        let name_sorter = CustomSorter::new(move |a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            if let Some(order) = folders_first(a, b, desc.get()) {
                return order;
            }
            let cmp = a.search_string().cmp(&b.search_string());
            match cmp {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            }
        });
        let name_column = ColumnViewColumn::builder()
            .title("Name")
            .factory(&name_factory)
            .sorter(&name_sorter)
            .expand(true)
            .resizable(true)
            .build();
        column_view.append_column(&name_column);

        // Size column with sorter
        let size_factory = SignalListItemFactory::new();
        setup_label_factory(&size_factory, "size-display");
        let desc = descending.clone();
        let size_sorter = CustomSorter::new(move |a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            if let Some(order) = folders_first(a, b, desc.get()) {
                return order;
            }
            match a.size().cmp(&b.size()) {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            }
        });
        let size_column = ColumnViewColumn::builder()
            .title("Size")
            .factory(&size_factory)
            .sorter(&size_sorter)
            .fixed_width(100)
            .resizable(true)
            .build();
        column_view.append_column(&size_column);

        // Modified column with sorter
        let modified_factory = SignalListItemFactory::new();
        setup_label_factory(&modified_factory, "modified-display");
        let desc = descending.clone();
        let modified_sorter = CustomSorter::new(move |a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            if let Some(order) = folders_first(a, b, desc.get()) {
                return order;
            }
            match a.modified().cmp(&b.modified()) {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            }
        });
        let modified_column = ColumnViewColumn::builder()
            .title("Date Modified")
            .factory(&modified_factory)
            .sorter(&modified_sorter)
            .fixed_width(160)
            .resizable(true)
            .build();
        column_view.append_column(&modified_column);

        // Kind column with sorter
        let kind_factory = SignalListItemFactory::new();
        setup_label_factory(&kind_factory, "file-type-name");
        let desc = descending.clone();
        let kind_sorter = CustomSorter::new(move |a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            if let Some(order) = folders_first(a, b, desc.get()) {
                return order;
            }
            let cmp = a
                .file_type_name()
                .to_lowercase()
                .cmp(&b.file_type_name().to_lowercase());
            match cmp {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
            }
        });
        let kind_column = ColumnViewColumn::builder()
            .title("Kind")
            .factory(&kind_factory)
            .sorter(&kind_sorter)
            .fixed_width(140)
            .resizable(true)
            .build();
        column_view.append_column(&kind_column);

        column_view.update_property(&[gtk::accessible::Property::Label("File list")]);

        // Build accessible header row — buttons that trigger sorting
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_row.add_css_class("toolbar");

        let columns = [
            ("Name", name_column.clone()),
            ("Size", size_column.clone()),
            ("Date Modified", modified_column.clone()),
            ("Kind", kind_column.clone()),
        ];

        // Track current sort column index and direction — restore from saved state
        let (saved_col, saved_asc) = crate::state::load_sort_state();
        let sort_col_idx: std::rc::Rc<std::cell::Cell<usize>> =
            std::rc::Rc::new(std::cell::Cell::new(saved_col as usize));
        let sort_ascending: std::rc::Rc<std::cell::Cell<bool>> =
            std::rc::Rc::new(std::cell::Cell::new(saved_asc));

        // Collect buttons so we can update their labels when sort changes
        let mut buttons_builder: Vec<(String, gtk::Button)> = Vec::new();

        for (i, (label_text, col)) in columns.iter().enumerate() {
            let btn = gtk::Button::builder().hexpand(col.expands()).build();
            if !col.expands() {
                btn.set_width_request(col.fixed_width());
            }

            buttons_builder.push((label_text.to_string(), btn.clone()));
            header_row.append(&btn);

            let cv = column_view.clone();
            let c = col.clone();
            let idx = sort_col_idx.clone();
            let asc = sort_ascending.clone();
            // We'll set the button references after building all buttons
            let _ = (i, cv, c, idx, asc);
        }

        let all_buttons = std::rc::Rc::new(buttons_builder);

        // Now connect click handlers with access to all buttons
        let mut child = header_row.first_child();
        for (i, (_label_text, col)) in columns.iter().enumerate() {
            if let Some(widget) = &child {
                let btn = widget.clone().downcast::<gtk::Button>().unwrap();
                let cv = column_view.clone();
                let c = col.clone();
                let idx = sort_col_idx.clone();
                let asc = sort_ascending.clone();
                let btns = all_buttons.clone();
                let desc = descending.clone();

                btn.connect_clicked(move |_| {
                    if idx.get() == i {
                        // Same column — toggle direction
                        asc.set(!asc.get());
                    } else {
                        idx.set(i);
                        asc.set(true);
                    }

                    let order = if asc.get() {
                        gtk::SortType::Ascending
                    } else {
                        gtk::SortType::Descending
                    };
                    desc.set(!asc.get());
                    cv.sort_by_column(Some(&c), order);
                    crate::state::save_sort_state(idx.get() as u32, asc.get());

                    // Update all button labels
                    let mut sorted_name = String::new();
                    for (j, (name, button)) in btns.iter().enumerate() {
                        if j == idx.get() {
                            let dir = if asc.get() { "ascending" } else { "descending" };
                            button.set_label(&format!("{name}, {dir}"));
                            button.update_property(&[gtk::accessible::Property::Label(&format!(
                                "{name}, sorted {dir}"
                            ))]);
                            sorted_name = name.clone();
                        } else {
                            button.set_label(name);
                            button.update_property(&[gtk::accessible::Property::Label(&format!(
                                "Sort by {name}"
                            ))]);
                        }
                    }

                    // Announce sort change for screen readers
                    let direction = if asc.get() { "ascending" } else { "descending" };
                    cv.announce(
                        &format!("Sorted by {sorted_name}, {direction}"),
                        gtk::AccessibleAnnouncementPriority::Medium,
                    );
                });

                child = widget.next_sibling();
            }
        }

        // Set initial labels based on saved sort state
        for (i, (name, btn)) in all_buttons.iter().enumerate() {
            if i == saved_col as usize {
                let dir = if saved_asc { "ascending" } else { "descending" };
                btn.set_label(&format!("{name}, {dir}"));
                btn.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{name}, sorted {dir}"
                ))]);
            } else {
                btn.set_label(name);
                btn.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Sort by {name}"
                ))]);
            }
        }

        // Sorting by clicking a column's own header: keep the buttons (what
        // Orca reads), the folders-first rule and the saved state in step.
        if let Some(view_sorter) = column_view.sorter().and_downcast::<gtk::ColumnViewSorter>() {
            let cols: Vec<ColumnViewColumn> = columns.iter().map(|(_, c)| c.clone()).collect();
            let sorters = [
                name_sorter.clone(),
                size_sorter.clone(),
                modified_sorter.clone(),
                kind_sorter.clone(),
            ];
            let idx = sort_col_idx.clone();
            let asc = sort_ascending.clone();
            let desc = descending.clone();
            let btns = all_buttons.clone();
            view_sorter.connect_changed(move |view_sorter, _| {
                let Some(col) = view_sorter.primary_sort_column() else {
                    return;
                };
                let Some(i) = cols.iter().position(|c| *c == col) else {
                    return;
                };
                let ascending = view_sorter.primary_sort_order() == gtk::SortType::Ascending;
                let flipped = desc.get() == ascending;
                idx.set(i);
                asc.set(ascending);
                desc.set(!ascending);
                crate::state::save_sort_state(i as u32, ascending);
                for (j, (name, button)) in btns.iter().enumerate() {
                    if j == i {
                        let dir = if ascending { "ascending" } else { "descending" };
                        button.set_label(&format!("{name}, {dir}"));
                        button.update_property(&[gtk::accessible::Property::Label(&format!(
                            "{name}, sorted {dir}"
                        ))]);
                    } else {
                        button.set_label(name);
                        button.update_property(&[gtk::accessible::Property::Label(&format!(
                            "Sort by {name}"
                        ))]);
                    }
                }
                if flipped {
                    // The folders-first rule depends on the direction.
                    for sorter in &sorters {
                        sorter.changed(gtk::SorterChange::Different);
                    }
                }
            });
        }

        // Down arrow from header buttons focuses the first file in the ColumnView
        let header_key_controller = gtk::EventControllerKey::new();
        header_key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cv = column_view.clone();
        header_key_controller.connect_key_pressed(move |_ctrl, key, _code, _mods| {
            if key == gtk::gdk::Key::Down {
                cv.grab_focus();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        header_row.add_controller(header_key_controller);

        // Put header + column view in a single box inside the scrolled window
        let inner_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        inner_box.append(&header_row);
        inner_box.append(&column_view);

        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&inner_box)
            .build();

        Self {
            column_view,
            scrolled_window,
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for ListViewInner {
    const NAME: &'static str = "WayfinderListView";
    type Type = super::WayfinderListView;
    type ParentType = glib::Object;
}

impl ObjectImpl for ListViewInner {}

fn setup_name_factory(factory: &SignalListItemFactory) {
    factory.connect_setup(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();

        let icon = gtk::Image::builder().pixel_size(16).margin_end(6).build();

        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        row.append(&icon);
        row.append(&label);

        let item_expr = gtk::ConstantExpression::new(item);
        let entry_expr =
            gtk::PropertyExpression::new(gtk::ListItem::static_type(), Some(&item_expr), "item");

        let icon_expr =
            gtk::PropertyExpression::new(FileObject::static_type(), Some(&entry_expr), "icon");
        icon_expr.bind(&icon, "icon-name", gtk::Widget::NONE);

        let name_expr =
            gtk::PropertyExpression::new(FileObject::static_type(), Some(&entry_expr), "name");
        name_expr.bind(&label, "label", gtk::Widget::NONE);

        // Accessible label binding — Nautilus pattern
        let a11y_item_expr = gtk::PropertyExpression::new(
            gtk::ListItem::static_type(),
            gtk::Expression::NONE,
            "item",
        );
        let a11y_name_expr = gtk::PropertyExpression::new(
            FileObject::static_type(),
            Some(&a11y_item_expr),
            "a11y-name",
        );
        a11y_name_expr.bind(item, "accessible-label", Some(item));

        // The row's own label (what Orca reads in context) follows the
        // file's details as they change — a download growing, say — not
        // just what they were when the row was bound.
        let row_weak = row.downgrade();
        let item_weak = item.downgrade();
        let label_expr = a11y_name_expr.clone();
        a11y_name_expr.watch(Some(item), move || {
            let (Some(row), Some(item)) = (row_weak.upgrade(), item_weak.upgrade()) else {
                return;
            };
            if let Some(text) = label_expr.evaluate_as::<String, _>(Some(&item)) {
                row.update_property(&[gtk::accessible::Property::Label(&text)]);
            }
        });

        crate::views::select_on_right_click(&row, item);
        item.set_child(Some(&row));
    });

    // Set accessible label on child widget for Orca context
    factory.connect_bind(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(file) = item.item().and_downcast::<FileObject>() {
            if let Some(child) = item.child() {
                // (Its accessible label follows the file: see the setup.)

                // Add DragSource for drag & drop
                let drag_source = gtk::DragSource::new();
                drag_source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
                let uri = format!("file://{}", file.path());
                let content = gdk::ContentProvider::for_value(&uri.to_value());
                drag_source.set_content(Some(&content));
                child.add_controller(drag_source);
            }
        }
    });

    // Remove drag controllers on unbind to prevent stacking on reuse
    factory.connect_unbind(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(child) = item.child() {
            let mut controllers_to_remove = Vec::new();
            let ctrl = child.observe_controllers();
            for i in 0..ctrl.n_items() {
                if let Some(c) = ctrl.item(i) {
                    if c.downcast_ref::<gtk::DragSource>().is_some() {
                        controllers_to_remove.push(c.downcast::<gtk::EventController>().unwrap());
                    }
                }
            }
            for ctrl in controllers_to_remove {
                child.remove_controller(&ctrl);
            }
        }
    });
}

fn setup_label_factory(factory: &SignalListItemFactory, property: &'static str) {
    factory.connect_setup(move |_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();

        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();

        let item_expr = gtk::ConstantExpression::new(item);
        let entry_expr =
            gtk::PropertyExpression::new(gtk::ListItem::static_type(), Some(&item_expr), "item");

        let prop_expr =
            gtk::PropertyExpression::new(FileObject::static_type(), Some(&entry_expr), property);
        prop_expr.bind(&label, "label", gtk::Widget::NONE);

        crate::views::select_on_right_click(&label, item);
        item.set_child(Some(&label));
    });
}

/// Folders before files, whichever way the list is sorted. The view
/// reverses every comparison for a descending sort, so the rule is
/// reversed first, to come out the right way round.
fn folders_first(a: &FileObject, b: &FileObject, descending: bool) -> Option<gtk::Ordering> {
    let (folder_first, file_first) = if descending {
        (gtk::Ordering::Larger, gtk::Ordering::Smaller)
    } else {
        (gtk::Ordering::Smaller, gtk::Ordering::Larger)
    };
    match (a.is_directory(), b.is_directory()) {
        (true, false) => Some(folder_first),
        (false, true) => Some(file_first),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::folders_first;
    use crate::file_object::{FileInfoData, FileObject};

    fn item(name: &str, is_directory: bool) -> FileObject {
        FileObject::from_data(
            "/d",
            &FileInfoData {
                name: name.to_string(),
                display_name: name.to_string(),
                is_directory,
                size: 0,
                modified: 0,
                modified_display: String::new(),
                content_type: String::new(),
            },
        )
    }

    /// The view reverses a descending comparison; the rule, reversed
    /// first, still puts folders first.
    #[test]
    fn folders_come_first_both_ways() {
        let (folder, file) = (item("b", true), item("a.txt", false));
        assert_eq!(
            folders_first(&folder, &file, false),
            Some(gtk::Ordering::Smaller)
        );
        assert_eq!(
            folders_first(&folder, &file, true),
            Some(gtk::Ordering::Larger)
        );
        assert_eq!(folders_first(&file, &item("c.txt", false), true), None);
    }
}
