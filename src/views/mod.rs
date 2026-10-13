pub mod grid_view;
pub mod list_view;

pub use grid_view::WayfinderGridView;
pub use list_view::WayfinderListView;

use gtk::prelude::*;

/// Right-clicking an item selects it first, so the context menu (opened by
/// the view's own right-click handler, which runs after this one) is for
/// the item clicked, not whatever was selected before.
pub(crate) fn select_on_right_click(child: &impl IsA<gtk::Widget>, item: &gtk::ListItem) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    let item = item.downgrade();
    gesture.connect_pressed(move |gesture, _, _, _| {
        let (Some(item), Some(widget)) = (item.upgrade(), gesture.widget()) else {
            return;
        };
        let pos = item.position();
        if pos == gtk::INVALID_LIST_POSITION {
            return;
        }
        let model = widget
            .ancestor(gtk::ColumnView::static_type())
            .and_downcast::<gtk::ColumnView>()
            .and_then(|v| v.model())
            .or_else(|| {
                widget
                    .ancestor(gtk::GridView::static_type())
                    .and_downcast::<gtk::GridView>()
                    .and_then(|v| v.model())
            });
        if let Some(selection) = model.and_downcast::<gtk::SingleSelection>() {
            selection.set_selected(pos);
        }
    });
    child.add_controller(gesture);
}
