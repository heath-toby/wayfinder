use gtk::prelude::*;

pub fn register_shortcuts(window: &super::window::WayfinderWindow) {
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Global);

    // Navigation shortcuts (actions are on the window, so prefix is "win.")
    add_shortcut(&controller, "<Ctrl><Shift>h", "win.home");
    add_shortcut(&controller, "<Ctrl><Shift>o", "win.documents");
    add_shortcut(&controller, "<Ctrl><Shift>k", "win.desktop");
    add_shortcut(&controller, "<Ctrl><Shift>l", "win.downloads");
    add_shortcut(&controller, "<Ctrl><Shift>r", "win.root");
    add_shortcut(&controller, "<Alt>Up", "win.up");
    add_shortcut(&controller, "<Alt>Left", "win.back");
    add_shortcut(&controller, "BackSpace", "win.up");
    add_shortcut(&controller, "<Alt>Right", "win.forward");
    add_shortcut(&controller, "<Ctrl>l", "win.location-bar");

    // View shortcuts
    add_shortcut(&controller, "<Ctrl>h", "win.toggle-hidden");
    add_shortcut(&controller, "<Ctrl>1", "win.view-grid");
    add_shortcut(&controller, "<Ctrl>2", "win.view-list");
    add_shortcut(&controller, "<Ctrl><Shift>s", "win.toggle-sidebar");

    // Search
    add_shortcut(&controller, "<Ctrl>f", "win.search");

    // File operations (global clipboard — works across windows)
    add_shortcut(&controller, "<Ctrl>c", "win.copy");
    add_shortcut(&controller, "<Ctrl>x", "win.cut");
    add_shortcut(&controller, "<Ctrl>v", "win.paste");
    // Window-local clipboard
    add_shortcut(&controller, "<Ctrl><Shift>c", "win.copy-local");
    add_shortcut(&controller, "<Ctrl><Shift>x", "win.cut-local");
    add_shortcut(&controller, "<Ctrl><Shift>v", "win.paste-local");
    add_shortcut(&controller, "Delete", "win.trash");
    add_shortcut(&controller, "<Shift>Delete", "win.delete");
    add_shortcut(&controller, "F2", "win.rename");
    add_shortcut(&controller, "<Ctrl><Shift>n", "win.new-folder");
    add_shortcut(&controller, "<Ctrl>a", "win.select-all");

    // Bookmarks
    add_shortcut(&controller, "<Ctrl>d", "win.bookmark");

    // Properties and context menu
    add_shortcut(&controller, "<Ctrl>i", "win.properties");
    add_shortcut(&controller, "<Shift>F10", "win.context-menu");
    add_shortcut(&controller, "Menu", "win.context-menu");

    // New window
    add_shortcut(&controller, "<Ctrl>n", "win.new-window");

    // Go to folder (same dialog as Ctrl+L)
    add_shortcut(&controller, "<Ctrl><Shift>g", "win.location-bar");

    // Shortcuts help
    add_shortcut(&controller, "<Ctrl>question", "win.show-shortcuts");

    window.add_controller(controller);
}

fn add_shortcut(controller: &gtk::ShortcutController, trigger_str: &str, action_name: &str) {
    let trigger = gtk::ShortcutTrigger::parse_string(trigger_str);
    let action = gtk::NamedAction::new(action_name);
    if let Some(trigger) = trigger {
        let shortcut = gtk::Shortcut::new(Some(trigger), Some(action));
        controller.add_shortcut(shortcut);
    }
}
