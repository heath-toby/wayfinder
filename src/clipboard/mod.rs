use std::cell::RefCell;

use gtk::gio;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOperation {
    Copy,
    Cut,
}

#[derive(Clone)]
pub struct ClipboardState {
    pub operation: ClipboardOperation,
    pub files: Vec<gio::File>,
}

impl ClipboardState {
    pub fn new(operation: ClipboardOperation, files: Vec<gio::File>) -> Self {
        Self { operation, files }
    }
}

// Global (app-wide) clipboard shared across all windows.
// Safe because GTK is single-threaded.
thread_local! {
    static GLOBAL_CLIPBOARD: RefCell<Option<ClipboardState>> = RefCell::new(None);
}

pub fn global_set(state: ClipboardState) {
    GLOBAL_CLIPBOARD.with(|c| *c.borrow_mut() = Some(state));
}

pub fn global_get() -> Option<ClipboardState> {
    GLOBAL_CLIPBOARD.with(|c| c.borrow().clone())
}

pub fn global_clear() {
    GLOBAL_CLIPBOARD.with(|c| *c.borrow_mut() = None);
}
