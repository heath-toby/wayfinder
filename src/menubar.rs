//! The menu bar (F10 opens it): Wayfinder, File, Edit, View, Go and Window.
//! Its items are the window's actions (`win.`, those the keyboard
//! shortcuts run) and the application's own (`app.`: About, Preferences,
//! Quit). Each item shows its shortcut; the shortcuts themselves are in
//! `shortcuts/mod.rs`.

use gtk::gio;
use gtk::prelude::*;

/// Give `app` its menu bar and the actions only it has.
pub fn install(app: &gtk::Application) {
    let about = gio::SimpleAction::new("about", None);
    let a = app.clone();
    about.connect_activate(move |_, _| show_about(a.active_window().as_ref()));
    app.add_action(&about);

    let preferences = gio::SimpleAction::new("preferences", None);
    let a = app.clone();
    preferences.connect_activate(move |_, _| {
        if let Some(parent) = a.active_window() {
            crate::preferences::show(&parent);
        }
    });
    app.add_action(&preferences);

    let quit = gio::SimpleAction::new("quit", None);
    let a = app.clone();
    quit.connect_activate(move |_, _| quit_app(&a));
    app.add_action(&quit);

    app.set_menubar(Some(&menu_bar()));
}

fn menu_bar() -> gio::Menu {
    let bar = gio::Menu::new();

    let app_menu = gio::Menu::new();
    app_menu.append_item(&item("_About Wayfinder", "app.about", None));
    app_menu.append_item(&item(
        "_Preferences…",
        "app.preferences",
        Some("<Ctrl>comma"),
    ));
    app_menu.append_item(&item(
        "_Keyboard Shortcuts",
        "win.show-shortcuts",
        Some("<Ctrl>question"),
    ));
    app_menu.append_section(
        None,
        &section(&[("_Quit Wayfinder", "app.quit", Some("<Ctrl>q"))]),
    );
    bar.append_submenu(Some("_Wayfinder"), &app_menu);

    let file = gio::Menu::new();
    file.append_section(
        None,
        &section(&[
            ("New _Window", "win.new-window", Some("<Ctrl>n")),
            ("New _Folder…", "win.new-folder", Some("<Ctrl><Shift>n")),
        ]),
    );
    file.append_section(
        None,
        &section(&[
            ("_Open", "win.open-selected", Some("Return")),
            (
                "Open _Terminal Here",
                "win.terminal-here",
                Some("<Ctrl>grave"),
            ),
            ("P_roperties", "win.properties", Some("<Ctrl>i")),
        ]),
    );
    file.append_section(
        None,
        &section(&[
            ("Re_name…", "win.rename", Some("F2")),
            (
                "Rename _Several…",
                "win.batch-rename",
                Some("<Ctrl><Shift>F2"),
            ),
            ("Add _Bookmark", "win.bookmark", Some("<Ctrl>d")),
        ]),
    );
    file.append_section(
        None,
        &section(&[
            ("_Move to Bin", "win.trash", Some("Delete")),
            ("_Delete Permanently…", "win.delete", Some("<Shift>Delete")),
            ("R_estore from Bin", "win.restore", None),
            ("Empty _Bin…", "win.empty-trash", None),
        ]),
    );
    file.append_section(
        None,
        &section(&[("_Close Window", "win.close-window", Some("<Ctrl>w"))]),
    );
    bar.append_submenu(Some("_File"), &file);

    let edit = gio::Menu::new();
    edit.append_section(
        None,
        &section(&[("_Undo Move to Bin", "win.undo", Some("<Ctrl>z"))]),
    );
    edit.append_section(
        None,
        &section(&[
            ("Cu_t", "win.cut", Some("<Ctrl>x")),
            ("_Copy", "win.copy", Some("<Ctrl>c")),
            ("Copy _Path", "win.copy-path", None),
            ("_Paste", "win.paste", Some("<Ctrl>v")),
        ]),
    );
    edit.append_submenu(
        Some("In This _Window Only"),
        &section(&[
            ("Cu_t", "win.cut-local", Some("<Ctrl><Shift>x")),
            ("_Copy", "win.copy-local", Some("<Ctrl><Shift>c")),
            ("_Paste", "win.paste-local", Some("<Ctrl><Shift>v")),
        ]),
    );
    edit.append_section(
        None,
        &section(&[
            ("Select _All", "win.select-all", Some("<Ctrl>a")),
            ("_Find…", "win.search", Some("<Ctrl>f")),
        ]),
    );
    bar.append_submenu(Some("_Edit"), &edit);

    let view = gio::Menu::new();
    view.append_section(
        None,
        &section(&[
            ("As _Icons", "win.view-mode::grid", Some("<Ctrl>1")),
            ("As _List", "win.view-mode::list", Some("<Ctrl>2")),
        ]),
    );
    let sort = gio::Menu::new();
    sort.append_section(
        None,
        &section(&[
            ("_Name", "win.sort-by::name", None),
            ("_Size", "win.sort-by::size", None),
            ("Date _Modified", "win.sort-by::modified", None),
            ("_Kind", "win.sort-by::kind", None),
        ]),
    );
    sort.append_section(
        None,
        &section(&[("_Descending", "win.sort-descending", None)]),
    );
    view.append_submenu(Some("_Sort By"), &sort);
    view.append_section(
        None,
        &section(&[
            ("Show _Hidden Files", "win.toggle-hidden", Some("<Ctrl>h")),
            ("Show Si_debar", "win.toggle-sidebar", Some("F3")),
            ("Show _Breadcrumb Bar", "win.toggle-breadcrumb", Some("F4")),
        ]),
    );
    view.append_section(
        None,
        &section(&[
            ("Zoom _In", "win.zoom-in", Some("<Ctrl>plus")),
            ("Zoom _Out", "win.zoom-out", Some("<Ctrl>minus")),
            ("_Actual Size", "win.zoom-reset", Some("<Ctrl>0")),
        ]),
    );
    bar.append_submenu(Some("_View"), &view);

    let go = gio::Menu::new();
    go.append_section(
        None,
        &section(&[
            ("_Back", "win.back", Some("<Alt>Left")),
            ("_Forward", "win.forward", Some("<Alt>Right")),
            ("_Enclosing Folder", "win.up", Some("<Alt>Up")),
        ]),
    );
    go.append_section(
        None,
        &section(&[
            ("_Home", "win.home", Some("<Ctrl><Shift>h")),
            ("_Desktop", "win.desktop", Some("<Ctrl><Shift>k")),
            ("D_ocuments", "win.documents", Some("<Ctrl><Shift>o")),
            ("Do_wnloads", "win.downloads", Some("<Ctrl><Shift>l")),
            ("_Music", "win.music", None),
            ("_Pictures", "win.pictures", None),
            ("_Videos", "win.videos", None),
        ]),
    );
    go.append_section(
        None,
        &section(&[
            ("_Recent Files", "win.recent", None),
            ("B_in", "win.open-trash", None),
            ("File _System", "win.root", Some("<Ctrl><Shift>r")),
        ]),
    );
    go.append_section(
        None,
        &section(&[("Go to _Location…", "win.location-bar", Some("<Ctrl>l"))]),
    );
    bar.append_submenu(Some("_Go"), &go);

    let window = gio::Menu::new();
    window.append_section(
        None,
        &section(&[
            ("New _Window", "win.new-window", Some("<Ctrl>n")),
            ("_Close Window", "win.close-window", Some("<Ctrl>w")),
        ]),
    );
    window.append_section(
        None,
        &section(&[("_Operations in Progress", "win.show-operations", None)]),
    );
    bar.append_submenu(Some("Wi_ndow"), &window);

    bar
}

/// A menu item for `action`, showing `accel` (only shown: the shortcut
/// itself is set up with the others).
fn item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

fn section(items: &[(&str, &str, Option<&str>)]) -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, action, accel) in items {
        menu.append_item(&item(label, action, *accel));
    }
    menu
}

fn show_about(parent: Option<&gtk::Window>) {
    let about = gtk::AboutDialog::builder()
        .program_name("Wayfinder")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("An accessible file manager, made to work with the Orca screen reader.")
        .website("https://github.com/heath-toby/wayfinder")
        .website_label("Wayfinder on GitHub")
        .license_type(gtk::License::Gpl30)
        .logo_icon_name("system-file-manager")
        .modal(true)
        .build();
    about.set_transient_for(parent);
    about.present();
}

/// Quit, closing every window (they're all reopened next time, if that's
/// on) — after asking, if operations are still running: quitting stops
/// them.
fn quit_app(app: &gtk::Application) {
    let running = wayfinder::operations::count();
    if running == 0 {
        app.quit();
        return;
    }
    let message = if running == 1 {
        "An operation is still running".to_string()
    } else {
        format!("{running} operations are still running")
    };
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail("Quitting now stops them, part-way through.")
        .buttons(["Don't Quit", "Quit Anyway"])
        .cancel_button(0)
        .default_button(0)
        .build();
    let a = app.clone();
    dialog.choose(
        app.active_window().as_ref(),
        gio::Cancellable::NONE,
        move |result| {
            if result == Ok(1) {
                a.quit();
            }
        },
    );
}
