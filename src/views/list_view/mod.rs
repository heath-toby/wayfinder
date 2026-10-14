mod imp;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{ListScrollFlags, SelectionModel};

glib::wrapper! {
    pub struct WayfinderListView(ObjectSubclass<imp::ListViewInner>);
}

impl WayfinderListView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_model(&self, model: &impl IsA<SelectionModel>) {
        self.imp().column_view.set_model(Some(model));
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.imp().scrolled_window
    }

    pub fn column_view(&self) -> &gtk::ColumnView {
        &self.imp().column_view
    }

    pub fn grab_focus_at_selected(&self, selected_pos: u32) {
        // scroll_to(FOCUS) only moves focus to a row widget that already
        // exists; straight after a load, or before the window is laid out,
        // it doesn't, and focus would stay wherever it was (e.g. the
        // sidebar). Focusing the list first makes sure it ends up here.
        self.imp().column_view.grab_focus();
        self.imp().column_view.scroll_to(
            selected_pos,
            None::<&gtk::ColumnViewColumn>,
            ListScrollFlags::FOCUS,
            None,
        );
    }

    /// Show the Original Location column (in the Bin) or not. Left sorted
    /// by it, the list goes back to sorting by name.
    pub fn show_original_location(&self, show: bool) {
        let imp = self.imp();
        imp.origin_column.set_visible(show);
        imp.origin_button.set_visible(show);
        if show {
            return;
        }
        let sorter = imp
            .column_view
            .sorter()
            .and_downcast::<gtk::ColumnViewSorter>();
        if sorter.and_then(|s| s.primary_sort_column()) == Some(imp.origin_column.clone()) {
            if let Some(name) = imp
                .column_view
                .columns()
                .item(0)
                .and_downcast::<gtk::ColumnViewColumn>()
            {
                imp.column_view
                    .sort_by_column(Some(&name), gtk::SortType::Ascending);
            }
        }
    }

    /// Whether sort changes are saved for next time (default: yes).
    pub fn set_persist_sort(&self, persist: bool) {
        self.imp().persist_sort.set(persist);
    }

    pub fn grab_focus(&self) {
        self.imp().column_view.grab_focus();
    }
}

impl Default for WayfinderListView {
    fn default() -> Self {
        Self::new()
    }
}
