use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{GridView, ScrolledWindow, SignalListItemFactory};

use crate::file_object::FileObject;

pub struct GridViewInner {
    pub grid_view: GridView,
    pub scrolled_window: ScrolledWindow,
}

impl Default for GridViewInner {
    fn default() -> Self {
        let factory = SignalListItemFactory::new();
        setup_factory(&factory);

        let grid_view = GridView::builder().min_columns(3).max_columns(12).build();
        grid_view.set_factory(Some(&factory));
        grid_view.update_property(&[gtk::accessible::Property::Label("Files")]);

        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&grid_view)
            .build();

        Self {
            grid_view,
            scrolled_window,
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for GridViewInner {
    const NAME: &'static str = "WayfinderGridView";
    type Type = super::WayfinderGridView;
    type ParentType = glib::Object;
}

impl ObjectImpl for GridViewInner {}

fn setup_factory(factory: &SignalListItemFactory) {
    // Add DragSource on bind
    factory.connect_bind(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(file) = item.item().and_downcast::<FileObject>() {
            if let Some(child) = item.child() {
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

    factory.connect_setup(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();

        let icon = gtk::Image::builder().pixel_size(48).build();

        let label = gtk::Label::builder()
            .xalign(0.5)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(14)
            .lines(2)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();

        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .halign(gtk::Align::Center)
            .build();
        vbox.append(&icon);
        vbox.append(&label);

        // Use ConstantExpression for visual bindings (icon, name) — these are fine
        let item_expr = gtk::ConstantExpression::new(item);
        let entry_expr =
            gtk::PropertyExpression::new(gtk::ListItem::static_type(), Some(&item_expr), "item");

        let icon_expr =
            gtk::PropertyExpression::new(FileObject::static_type(), Some(&entry_expr), "icon");
        icon_expr.bind(&icon, "icon-name", gtk::Widget::NONE);

        let name_expr =
            gtk::PropertyExpression::new(FileObject::static_type(), Some(&entry_expr), "name");
        name_expr.bind(&label, "label", gtk::Widget::NONE);

        crate::views::select_on_right_click(&vbox, item);
        item.set_child(Some(&vbox));

        // Accessible label binding — use the exact Nautilus pattern:
        // Expression::NONE as `this`, bind with listitem as the `this` object.
        // This creates a dynamic expression that re-evaluates when the ListItem
        // is recycled and rebound to a new data item.
        //   listitem:accessible-label := listitem:item:a11y-name
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
    });
}
