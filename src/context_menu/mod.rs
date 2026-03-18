use std::rc::Rc;

use gtk::gio;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{AccessibleAnnouncementPriority, AccessibleRole};

use crate::window::WayfinderWindow;

/// Create a Box with the Menu accessible role.
fn menu_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .accessible_role(AccessibleRole::Menu)
        .css_classes(["menu"])
        .build()
}

/// Collect all focusable Button children from a Box, skipping separators.
fn get_buttons(container: &gtk::Box) -> Vec<gtk::Button> {
    let mut buttons = Vec::new();
    let mut child = container.first_child();
    while let Some(widget) = child {
        if let Ok(btn) = widget.clone().downcast::<gtk::Button>() {
            buttons.push(btn);
        }
        child = widget.next_sibling();
    }
    buttons
}

/// Find which button in the list currently has focus, returning its index.
fn focused_index(buttons: &[gtk::Button]) -> Option<usize> {
    buttons.iter().position(|b| b.has_focus() || b.is_focus())
}

/// Focus the button at the given index.
fn focus_button(buttons: &[gtk::Button], idx: usize) {
    if let Some(btn) = buttons.get(idx) {
        btn.grab_focus();
    }
}

/// Install arrow-key navigation on a menu box.
/// - Up/Down: move between items, wrapping at ends
/// - Right: if on_right is Some, call it (enter submenu)
/// - Left: if on_left is Some, call it (leave submenu)
fn install_menu_nav(
    container: &gtk::Box,
    on_right: Option<Box<dyn Fn() + 'static>>,
    on_left: Option<Box<dyn Fn() + 'static>>,
) {
    let key_ctrl = gtk::EventControllerKey::new();
    let c = container.clone();
    key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
        let buttons = get_buttons(&c);
        if buttons.is_empty() {
            return glib::Propagation::Proceed;
        }

        let current = focused_index(&buttons).unwrap_or(0);

        match keyval {
            gdk::Key::Up => {
                let next = if current == 0 {
                    buttons.len() - 1
                } else {
                    current - 1
                };
                focus_button(&buttons, next);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                let next = if current >= buttons.len() - 1 {
                    0
                } else {
                    current + 1
                };
                focus_button(&buttons, next);
                glib::Propagation::Stop
            }
            gdk::Key::Right => {
                if let Some(ref cb) = on_right {
                    cb();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            gdk::Key::Left => {
                if let Some(ref cb) = on_left {
                    cb();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            _ => glib::Propagation::Proceed,
        }
    });
    container.add_controller(key_ctrl);
}

pub fn show_context_menu(window: &WayfinderWindow, x: f64, y: f64) {
    let in_trash = window.is_in_trash();

    let Some(file) = window.get_selected_file() else {
        show_background_menu(window, x, y, in_trash);
        return;
    };

    let popover = gtk::Popover::new();
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);

    // === Main menu page ===
    let main_box = menu_box();

    if in_trash {
        add_menu_item(&main_box, "Restore", &popover, {
            let w = window.clone();
            move || w.restore_selected()
        });
        add_menu_item(&main_box, "Delete Permanently", &popover, {
            let w = window.clone();
            move || w.delete_selected()
        });
        add_separator(&main_box);
        add_menu_item(&main_box, "Empty Bin", &popover, {
            let w = window.clone();
            move || w.empty_trash()
        });

        install_menu_nav(&main_box, None, None);
    } else {
        add_menu_item(&main_box, "Open", &popover, {
            let w = window.clone();
            let f = file.clone();
            move || {
                if f.is_directory() {
                    w.navigate_to_path(&f.path());
                } else {
                    w.open_file(&f);
                }
            }
        });

        // "Open With >" submenu trigger
        let content_type = file.mime_type();
        let apps = if content_type.is_empty() {
            Vec::new()
        } else {
            gio::AppInfo::all_for_type(&content_type)
                .into_iter()
                .take(10)
                .collect::<Vec<_>>()
        };

        let has_submenu = !apps.is_empty();

        if has_submenu {
            let submenu_btn = gtk::Button::builder()
                .accessible_role(AccessibleRole::MenuItem)
                .css_classes(["flat"])
                .build();
            submenu_btn.update_property(&[
                gtk::accessible::Property::Label("Open With, submenu"),
            ]);
            let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let label = gtk::Label::new(Some("Open With"));
            label.set_hexpand(true);
            label.set_halign(gtk::Align::Start);
            let arrow = gtk::Label::new(Some("\u{203A}")); // ›
            btn_box.append(&label);
            btn_box.append(&arrow);
            submenu_btn.set_child(Some(&btn_box));
            let s = stack.clone();
            submenu_btn.connect_clicked(move |_| {
                s.set_visible_child_name("open-with");
            });
            main_box.append(&submenu_btn);
        }

        add_separator(&main_box);

        add_menu_item(&main_box, "Cut", &popover, {
            let w = window.clone();
            move || w.cut_selected()
        });
        add_menu_item(&main_box, "Copy", &popover, {
            let w = window.clone();
            move || w.copy_selected()
        });
        add_menu_item(&main_box, "Copy Path", &popover, {
            let w = window.clone();
            let f = file.clone();
            move || {
                let path = f.path();
                let display = WidgetExt::display(&w);
                let clipboard = display.clipboard();
                clipboard.set_text(&path);
                w.announce(
                    &format!("Copied path: {}", path),
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
            }
        });
        add_menu_item(&main_box, "Copy Name", &popover, {
            let w = window.clone();
            let f = file.clone();
            move || {
                let name = f.name();
                let display = WidgetExt::display(&w);
                let clipboard = display.clipboard();
                clipboard.set_text(&name);
                w.announce(
                    &format!("Copied name: {}", name),
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
            }
        });
        add_menu_item(&main_box, "Paste", &popover, {
            let w = window.clone();
            move || w.paste()
        });

        add_separator(&main_box);

        add_menu_item(&main_box, "Rename", &popover, {
            let w = window.clone();
            move || w.rename_selected()
        });
        add_menu_item(&main_box, "Move to Bin", &popover, {
            let w = window.clone();
            move || w.trash_selected()
        });
        add_menu_item(&main_box, "Delete Permanently", &popover, {
            let w = window.clone();
            move || w.delete_selected()
        });

        add_separator(&main_box);

        add_menu_item(&main_box, "Properties", &popover, {
            let w = window.clone();
            move || {
                let win = w.clone();
                glib::idle_add_local_once(move || {
                    win.show_properties();
                });
            }
        });

        // Custom actions from .desktop files and Nautilus scripts
        let custom_actions = wayfinder::actions::load_actions();
        let matching_actions: Vec<_> = custom_actions
            .into_iter()
            .filter(|a| wayfinder::actions::matches_mime(a, &content_type))
            .collect();

        if !matching_actions.is_empty() {
            add_separator(&main_box);

            for action in matching_actions {
                let action = Rc::new(action);
                let action_name = action.name.clone();
                if wayfinder::actions::is_compress_dialog(&action) {
                    add_menu_item(&main_box, &action_name, &popover, {
                        let w = window.clone();
                        let f = file.clone();
                        move || {
                            let parent: gtk::Window = w.clone().upcast();
                            wayfinder::actions::show_compress_dialog(&[f.path()], &parent);
                        }
                    });
                } else {
                    add_menu_item(&main_box, &action_name, &popover, {
                        let w = window.clone();
                        let f = file.clone();
                        let a = action.clone();
                        move || {
                            let file_path = f.path();
                            let current_dir = w.imp().model.current_path();
                            wayfinder::actions::execute_action(&a, &[file_path], &current_dir);
                        }
                    });
                }
            }
        }

        // === Open With submenu page ===
        if has_submenu {
            let sub_box = menu_box();

            for app in &apps {
                let name = app.name().to_string();
                add_menu_item(&sub_box, &name, &popover, {
                    let w = window.clone();
                    let f = file.clone();
                    let a = app.clone();
                    move || {
                        let gio_file = gio::File::for_path(f.path());
                        let ctx = WidgetExt::display(&w).app_launch_context();
                        if let Err(e) = a.launch(&[gio_file], Some(&ctx)) {
                            w.announce(
                                &format!("Failed to open with {}: {}", a.name(), e),
                                AccessibleAnnouncementPriority::High,
                            );
                        }
                    }
                });
            }

            // Arrow nav: Left goes back to main, focus the "Open With" trigger
            let s_back = stack.clone();
            let mb = main_box.clone();
            install_menu_nav(
                &sub_box,
                None,
                Some(Box::new(move || {
                    s_back.set_visible_child_name("main");
                    // Focus the "Open With" button in the main menu
                    let buttons = get_buttons(&mb);
                    // It's the second button (after "Open")
                    if let Some(btn) = buttons.get(1) {
                        btn.grab_focus();
                    }
                })),
            );

            stack.add_named(&sub_box, Some("open-with"));

            // Arrow nav for main menu: Right on any item enters submenu
            let s_fwd = stack.clone();
            let sb = sub_box.clone();
            install_menu_nav(
                &main_box,
                Some(Box::new(move || {
                    s_fwd.set_visible_child_name("open-with");
                    // Focus first item in submenu
                    let buttons = get_buttons(&sb);
                    focus_button(&buttons, 0);
                })),
                None,
            );
        } else {
            install_menu_nav(&main_box, None, None);
        }
    }

    stack.add_named(&main_box, Some("main"));
    stack.set_visible_child_name("main");

    popover.set_child(Some(&stack));

    let parent_widget = window.active_view_widget();
    popover.set_parent(&parent_widget);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.set_has_arrow(false);

    // Focus the first item when the popover opens
    let mb = main_box.clone();
    popover.connect_show(move |_| {
        let buttons = get_buttons(&mb);
        focus_button(&buttons, 0);
    });

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

fn add_menu_item(
    container: &gtk::Box,
    label: &str,
    popover: &gtk::Popover,
    callback: impl Fn() + 'static,
) {
    let button = gtk::Button::builder()
        .label(label)
        .accessible_role(AccessibleRole::MenuItem)
        .css_classes(["flat"])
        .halign(gtk::Align::Fill)
        .build();
    let pop = popover.clone();
    button.connect_clicked(move |_| {
        pop.popdown();
        callback();
    });
    container.append(&button);
}

fn add_separator(container: &gtk::Box) {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    container.append(&sep);
}

fn show_background_menu(window: &WayfinderWindow, x: f64, y: f64, in_trash: bool) {
    let popover = gtk::Popover::new();
    let vbox = menu_box();

    if in_trash {
        add_menu_item(&vbox, "Empty Bin", &popover, {
            let w = window.clone();
            move || w.empty_trash()
        });
    } else {
        add_menu_item(&vbox, "Paste", &popover, {
            let w = window.clone();
            move || w.paste()
        });
        add_menu_item(&vbox, "New Folder", &popover, {
            let w = window.clone();
            move || w.create_new_folder()
        });
    }

    popover.set_child(Some(&vbox));
    install_menu_nav(&vbox, None, None);

    let parent_widget = window.active_view_widget();
    popover.set_parent(&parent_widget);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.set_has_arrow(false);

    let vb = vbox.clone();
    popover.connect_show(move |_| {
        let buttons = get_buttons(&vb);
        focus_button(&buttons, 0);
    });

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

pub fn register_open_with_actions(window: &WayfinderWindow) {
    // Open selected (still needed for keyboard shortcut / double-click)
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
