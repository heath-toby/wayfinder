//! Preferences (Wayfinder > Preferences, Ctrl+comma): General, and which
//! places the sidebar shows, in pages (Ctrl+Page Down and Ctrl+Page Up go
//! between them).

use gtk::glib;
use gtk::prelude::*;

thread_local! {
    /// The Preferences window, if open: asked for again, it's brought back.
    static OPEN: std::cell::RefCell<glib::WeakRef<gtk::Window>> =
        std::cell::RefCell::new(glib::WeakRef::new());
}

pub fn show(parent: &gtk::Window) {
    if let Some(open) = OPEN.with(|o| o.borrow().upgrade()) {
        open.present();
        return;
    }

    let dlg = gtk::Window::builder()
        .title("Preferences")
        .modal(true)
        .transient_for(parent)
        .default_width(450)
        .default_height(500)
        .build();
    OPEN.with(|o| o.borrow().set(Some(&dlg)));

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);

    // General
    let general = gtk::Box::new(gtk::Orientation::Vertical, 8);
    general.set_margin_top(12);
    general.set_margin_bottom(12);
    general.set_margin_start(12);
    general.set_margin_end(12);
    let startup = gtk::Label::builder()
        .label("When Wayfinder starts")
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    general.append(&startup);
    let reopen = wayfinder::sidebar::settings_check();
    reopen.update_property(&[gtk::accessible::Property::Description(
        "Every window open when Wayfinder last closed opens again, at the folder it showed",
    )]);
    general.append(&reopen);
    notebook.append_page(&general, Some(&gtk::Label::new(Some("General"))));

    // Sidebar
    let (places, places_list, edit) = wayfinder::sidebar::places_editor();
    let heading = gtk::Label::builder()
        .label("Show these places in the sidebar")
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    places.prepend(&heading);
    notebook.append_page(&places, Some(&gtk::Label::new(Some("Sidebar"))));

    let close_btn = gtk::Button::with_mnemonic("_Close");
    close_btn.set_halign(gtk::Align::End);
    close_btn.set_margin_top(8);
    close_btn.set_margin_bottom(12);
    close_btn.set_margin_end(12);
    let d = dlg.clone();
    close_btn.connect_clicked(move |_| d.close());

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&notebook);
    vbox.append(&close_btn);
    dlg.set_child(Some(&vbox));

    // Escape closes it, as Close does.
    let keys = gtk::EventControllerKey::new();
    let d = dlg.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            d.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    dlg.add_controller(keys);

    // The sidebar's places are kept, and shown in every window, when it
    // closes (the General settings are kept as they're changed).
    dlg.connect_close_request(move |_| {
        wayfinder::sidebar::save_places(&edit);
        glib::Propagation::Proceed
    });

    // The Sidebar page's list starts on its first place.
    if let Some(first) = places_list.row_at_index(0) {
        places_list.select_row(Some(&first));
    }

    dlg.present();
    reopen.grab_focus();
}
