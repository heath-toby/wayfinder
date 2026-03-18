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
            .show_column_separators(true)
            .build();

        // Hide native column headers — we provide our own accessible ones
        column_view.set_show_column_separators(true);

        // Name column with sorter
        let name_factory = SignalListItemFactory::new();
        setup_name_factory(&name_factory);
        let name_sorter = CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            match (a.is_directory(), b.is_directory()) {
                (true, false) => return gtk::Ordering::Smaller,
                (false, true) => return gtk::Ordering::Larger,
                _ => {}
            }
            let cmp = a.name().to_lowercase().cmp(&b.name().to_lowercase());
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
        let size_sorter = CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            match (a.is_directory(), b.is_directory()) {
                (true, false) => return gtk::Ordering::Smaller,
                (false, true) => return gtk::Ordering::Larger,
                _ => {}
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
        let modified_sorter = CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            match (a.is_directory(), b.is_directory()) {
                (true, false) => return gtk::Ordering::Smaller,
                (false, true) => return gtk::Ordering::Larger,
                _ => {}
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
        let kind_sorter = CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<FileObject>().unwrap();
            let b = b.downcast_ref::<FileObject>().unwrap();
            match (a.is_directory(), b.is_directory()) {
                (true, false) => return gtk::Ordering::Smaller,
                (false, true) => return gtk::Ordering::Larger,
                _ => {}
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
            let btn = gtk::Button::builder()
                .hexpand(col.expands())
                .build();
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
                    cv.sort_by_column(Some(&c), order);
                    crate::state::save_sort_state(idx.get() as u32, asc.get());

                    // Update all button labels
                    let mut sorted_name = String::new();
                    for (j, (name, button)) in btns.iter().enumerate() {
                        if j == idx.get() {
                            let dir = if asc.get() { "ascending" } else { "descending" };
                            button.set_label(&format!("{}, {}", name, dir));
                            button.update_property(&[gtk::accessible::Property::Label(
                                &format!("{}, sorted {}", name, dir),
                            )]);
                            sorted_name = name.clone();
                        } else {
                            button.set_label(name);
                            button.update_property(&[gtk::accessible::Property::Label(
                                &format!("Sort by {}", name),
                            )]);
                        }
                    }

                    // Announce sort change for screen readers
                    let direction = if asc.get() { "ascending" } else { "descending" };
                    cv.announce(
                        &format!("Sorted by {}, {}", sorted_name, direction),
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
                btn.set_label(&format!("{}, {}", name, dir));
                btn.update_property(&[gtk::accessible::Property::Label(
                    &format!("{}, sorted {}", name, dir),
                )]);
            } else {
                btn.set_label(name);
                btn.update_property(&[gtk::accessible::Property::Label(
                    &format!("Sort by {}", name),
                )]);
            }
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

        let icon_expr = gtk::PropertyExpression::new(
            FileObject::static_type(),
            Some(&entry_expr),
            "icon",
        );
        icon_expr.bind(&icon, "icon-name", gtk::Widget::NONE);

        let name_expr = gtk::PropertyExpression::new(
            FileObject::static_type(),
            Some(&entry_expr),
            "name",
        );
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

        item.set_child(Some(&row));
    });

    // Set accessible label on child widget for Orca context
    factory.connect_bind(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(file) = item.item().and_downcast::<FileObject>() {
            if let Some(child) = item.child() {
                let label = if file.is_directory() {
                    format!("{}, Folder", file.name())
                } else {
                    format!(
                        "{}, {}, {}",
                        file.name(),
                        file.size_display(),
                        file.file_type_name()
                    )
                };
                child.update_property(&[gtk::accessible::Property::Label(&label)]);

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

        let prop_expr = gtk::PropertyExpression::new(
            FileObject::static_type(),
            Some(&entry_expr),
            property,
        );
        prop_expr.bind(&label, "label", gtk::Widget::NONE);

        item.set_child(Some(&label));
    });
}
