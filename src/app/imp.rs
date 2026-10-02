use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::window::WayfinderWindow;

#[derive(Default)]
pub struct WayfinderApplicationInner;

#[glib::object_subclass]
impl ObjectSubclass for WayfinderApplicationInner {
    const NAME: &'static str = "WayfinderApplication";
    type Type = super::WayfinderApplication;
    type ParentType = gtk::Application;
}

impl ObjectImpl for WayfinderApplicationInner {}

impl ApplicationImpl for WayfinderApplicationInner {
    fn startup(&self) {
        self.parent_startup();
        let app = self.obj().clone();
        let rx = crate::dbus::start_service();
        crate::dbus::connect_to_app(app.upcast_ref(), rx);

        // Record any rclone FUSE mountpoints we can see right now. The list
        // persists across sessions so cache fallback still works after the
        // mount is unmounted.
        for mp in wayfinder::rclone_ops::current_rclone_fuse_mountpoints() {
            wayfinder::offline::record_mountpoint(&mp);
        }

        // Refresh offline pins in the background. Reachable pins get their
        // cache copies updated; unreachable ones are skipped.
        std::thread::spawn(|| {
            let _ = wayfinder::offline::sync_all();
        });
    }

    fn activate(&self) {
        let app = self.obj();

        // Restore previous session windows
        let session = wayfinder::state::load_window_session();
        if session.len() > 1 {
            wayfinder::state::clear_window_session();
            for dir in &session {
                let window = WayfinderWindow::new(app.upcast_ref());
                window.navigate_to_path(&dir.to_string_lossy());
                window.present();
            }
        } else {
            // Single window or no session — normal startup
            // (WayfinderWindow::new already loads last_directory)
            wayfinder::state::clear_window_session();
            let window = WayfinderWindow::new(app.upcast_ref());
            window.present();
        }
    }

    fn open(&self, files: &[gio::File], _hint: &str) {
        let app = self.obj();

        for file in files {
            let window = WayfinderWindow::new(app.upcast_ref());

            if let Some(path) = file.path() {
                let path_str = path.to_string_lossy().to_string();
                if path.is_dir() {
                    // It's a directory — navigate to it
                    window.navigate_to_path(&path_str);
                } else if path.is_file() {
                    // It's a file — navigate to its parent and open it
                    if let Some(parent) = path.parent() {
                        window.navigate_to_path(&parent.to_string_lossy());
                    }
                    // Open the file with its associated app
                    if let Some(file_obj) = window.get_selected_file() {
                        window.open_file(&file_obj);
                    }
                } else {
                    // Path doesn't exist yet — try navigating (will walk up)
                    window.navigate_to_path(&path_str);
                }
            }

            window.present();
        }
    }

    fn shutdown(&self) {
        // Save all open window directories for session restore
        let app = self.obj();
        let mut dirs = Vec::new();
        for window in app.windows() {
            if let Ok(wf) = window.downcast::<WayfinderWindow>() {
                let path = wf.imp().model.current_path();
                if !path.is_empty() {
                    dirs.push(path);
                }
            }
        }
        if !dirs.is_empty() {
            wayfinder::state::save_window_session(&dirs);
        }
        self.parent_shutdown();
    }
}

impl GtkApplicationImpl for WayfinderApplicationInner {}
