mod app;
mod context_menu;
mod dbus;
mod menubar;
mod preferences;
mod properties;
mod shortcuts;
mod window;

use gtk::prelude::*;

use app::WayfinderApplication;

fn main() {
    env_logger::init();

    let app = WayfinderApplication::new();
    app.run();
}
