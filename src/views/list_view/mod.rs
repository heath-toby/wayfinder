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
