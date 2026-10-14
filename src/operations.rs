//! Operations in progress (copies, moves, deletes, downloads, syncs,
//! actions): their progress windows. Closing one cancels its operation (each
//! window sees to that); its "Continue in Background" button only hides it,
//! and Window > Operations in Progress brings it back.

use gtk::glib;
use gtk::prelude::*;

thread_local! {
    /// Progress windows whose operation is still running.
    static RUNNING: std::cell::RefCell<Vec<glib::WeakRef<gtk::Window>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `window` shows an operation now running. Call `finished` or `close`
/// when it ends.
pub fn register(window: &gtk::Window) {
    RUNNING.with(|r| r.borrow_mut().push(window.downgrade()));
}

/// A "Continue in Background" button for `window`: it hides the window,
/// saying (from `parent`) how to get it back. The work goes on.
pub fn background_button(window: &gtk::Window, parent: &gtk::Window) -> gtk::Button {
    let button = gtk::Button::with_mnemonic("Continue in _Background");
    button.update_property(&[gtk::accessible::Property::Description(
        "Hides this window while the work goes on. Window, Operations in Progress shows it again.",
    )]);
    let window = window.downgrade();
    let parent = parent.downgrade();
    button.connect_clicked(move |_| {
        if let (Some(window), Some(parent)) = (window.upgrade(), parent.upgrade()) {
            hide(&window, &parent);
        }
    });
    button
}

/// Hide `window` while its operation goes on, saying (from `parent`) how
/// to get it back.
pub fn hide(window: &gtk::Window, parent: &gtk::Window) {
    window.set_visible(false);
    parent.announce(
        "Continuing in the background. To see it again, choose Operations in Progress from the Window menu.",
        gtk::AccessibleAnnouncementPriority::Medium,
    );
}

/// The operation `window` shows has ended. If its window was hidden, it
/// goes (the result is said from the window behind); otherwise it stays
/// for its own Close.
pub fn finished(window: &gtk::Window) {
    forget(window);
    if !window.is_visible() {
        window.close();
    }
}

/// The operation `window` shows has ended, and the window goes too.
pub fn close(window: &gtk::Window) {
    forget(window);
    window.close();
}

fn forget(window: &gtk::Window) {
    RUNNING.with(|r| {
        r.borrow_mut()
            .retain(|w| w.upgrade().is_some_and(|w| w != *window))
    });
}

/// Whether `window`'s operation is still running.
pub fn is_running(window: &gtk::Window) -> bool {
    RUNNING.with(|r| {
        r.borrow()
            .iter()
            .any(|w| w.upgrade().is_some_and(|w| w == *window))
    })
}

/// How many operations are running.
pub fn count() -> usize {
    RUNNING.with(|r| {
        r.borrow_mut().retain(|w| w.upgrade().is_some());
        r.borrow().len()
    })
}

/// Show every running operation's progress window again; how many there
/// are. (The last one shown has focus.)
pub fn show_all() -> usize {
    let windows: Vec<gtk::Window> =
        RUNNING.with(|r| r.borrow().iter().filter_map(|w| w.upgrade()).collect());
    for window in &windows {
        window.present();
    }
    windows.len()
}
