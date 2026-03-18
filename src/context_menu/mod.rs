use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use wayfinder::file_object::FileObject;
use crate::window::WayfinderWindow;

pub fn show_context_menu(window: &WayfinderWindow, x: f64, y: f64) {
    let in_trash = window.is_in_trash();

    let Some(file) = window.get_selected_file() else {
        show_background_menu(window, x, y, in_trash);
        return;
    };

    let menu = gio::Menu::new();

    if in_trash {
        menu.append(Some("Restore"), Some("win.restore"));
        menu.append(Some("Delete Permanently"), Some("win.delete"));

        let section2 = gio::Menu::new();
        section2.append(Some("Empty Bin"), Some("win.empty-trash"));
        menu.append_section(None, &section2);
    } else {
        menu.append(Some("Open"), Some("win.open-selected"));

        let open_with_menu = build_open_with_menu(&file);
        menu.append_submenu(Some("Open With"), &open_with_menu);

        menu.append(Some("Cut"), Some("win.cut"));
        menu.append(Some("Copy"), Some("win.copy"));
        menu.append(Some("Paste"), Some("win.paste"));

        let section2 = gio::Menu::new();
        section2.append(Some("Rename"), Some("win.rename"));
        section2.append(Some("Move to Bin"), Some("win.trash"));
        section2.append(Some("Delete Permanently"), Some("win.delete"));
        menu.append_section(None, &section2);

        let section3 = gio::Menu::new();
        section3.append(Some("Properties"), Some("win.properties"));
        menu.append_section(None, &section3);
    }

    show_popover(window, &menu, x, y);
}

fn show_background_menu(window: &WayfinderWindow, x: f64, y: f64, in_trash: bool) {
    let menu = gio::Menu::new();

    if in_trash {
        menu.append(Some("Empty Bin"), Some("win.empty-trash"));
    } else {
        menu.append(Some("Paste"), Some("win.paste"));
        menu.append(Some("New Folder"), Some("win.new-folder"));
    }

    show_popover(window, &menu, x, y);
}

fn show_popover(window: &WayfinderWindow, menu: &gio::Menu, x: f64, y: f64) {
    let popover = gtk::PopoverMenu::from_model(Some(menu));

    // Parent to the active view widget so GTK restores focus there on close
    let parent_widget = window.active_view_widget();
    popover.set_parent(&parent_widget);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.set_has_arrow(false);

    let w = window.clone();
    popover.connect_closed(move |pop| {
        pop.unparent();
        let win = w.clone();
        glib::idle_add_local_once(move || {
            win.restore_focus_to_selected();
        });
    });

    popover.popup();
}

fn build_open_with_menu(file: &FileObject) -> gio::Menu {
    let menu = gio::Menu::new();
    let content_type = file.mime_type();

    if content_type.is_empty() {
        return menu;
    }

    let apps = gio::AppInfo::all_for_type(&content_type);
    for (i, app) in apps.iter().enumerate().take(10) {
        let name = app.name().to_string();
        let action_name = format!("win.open-with-{}", i);
        menu.append(Some(&name), Some(&action_name));
    }

    menu
}

pub fn register_open_with_actions(window: &WayfinderWindow) {
    // Open selected
    let w = window.clone();
    let action = gio::SimpleAction::new("open-selected", None);
    action.connect_activate(move |_, _| {
        if let Some(file) = w.get_selected_file() {
            if file.is_directory() {
                w.navigate_to_path(&file.path());
            } else {
                w.open_file(&file);
            }
        }
    });
    window.add_action(&action);

    // Open with N
    for i in 0..10 {
        let w = window.clone();
        let action = gio::SimpleAction::new(&format!("open-with-{}", i), None);
        action.connect_activate(move |_, _| {
            if let Some(file) = w.get_selected_file() {
                let content_type = file.mime_type();
                let apps = gio::AppInfo::all_for_type(&content_type);
                if let Some(app) = apps.get(i) {
                    let gio_file = gio::File::for_path(file.path());
                    if let Err(e) = app.launch(&[gio_file], gio::AppLaunchContext::NONE) {
                        w.announce(
                            &format!("Failed to open with {}: {}", app.name(), e),
                            AccessibleAnnouncementPriority::High,
                        );
                    }
                }
            }
        });
        window.add_action(&action);
    }

    // Properties — defer so the popover closes first
    let w = window.clone();
    let action = gio::SimpleAction::new("properties", None);
    action.connect_activate(move |_, _| {
        let win = w.clone();
        glib::idle_add_local_once(move || {
            win.show_properties();
        });
    });
    window.add_action(&action);

    // Restore from bin
    let w = window.clone();
    let action = gio::SimpleAction::new("restore", None);
    action.connect_activate(move |_, _| {
        w.restore_selected();
    });
    window.add_action(&action);

    // Empty bin
    let w = window.clone();
    let action = gio::SimpleAction::new("empty-trash", None);
    action.connect_activate(move |_, _| {
        w.empty_trash();
    });
    window.add_action(&action);
}
