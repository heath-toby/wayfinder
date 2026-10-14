use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::window::WayfinderWindow;
use wayfinder::file_object::FileObject;

#[derive(Default)]
pub struct WayfinderApplicationInner {
    /// The last session has been reopened (only ever at the first start:
    /// launching again while running just opens a window).
    restored: std::cell::Cell<bool>,
}

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
        crate::menubar::install(app.upcast_ref());
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

        // Open every window of the last session again (unless that's
        // turned off in the sidebar's menu).
        let session = wayfinder::state::load_window_session();
        let first_start = !self.restored.replace(true);
        if first_start && wayfinder::state::load_reopen_windows() && session.len() > 1 {
            wayfinder::state::clear_window_session();
            for dir in &session {
                let window = WayfinderWindow::new(app.upcast_ref());
                let location = dir.to_string_lossy();
                if location.contains("://") {
                    window.load_special_uri(&location);
                } else {
                    window.navigate_to_path(&location);
                }
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
        // Opened with something to show: this run's start, too — a later
        // plain launch mustn't reopen the last session over it.
        self.restored.set(true);

        for file in files {
            let window = WayfinderWindow::new(app.upcast_ref());

            if let Some(path) = file.path() {
                let path_str = path.to_string_lossy().to_string();
                // Don't stat it here: on a slow mount (or through a link
                // to one) that would hold up startup. Navigating resolves
                // a file to its folder; whether to open it — with its
                // per-file app, if one was chosen — is checked in the
                // background.
                window.navigate_to_path(&path_str);
                let probe = path_str.clone();
                window.run_blocking(
                    &path_str,
                    move || {
                        // Following any link: a link to a folder was opened by
                        // navigating already.
                        let is_folder = std::fs::metadata(&probe).is_ok_and(|m| m.is_dir());
                        (!is_folder)
                            .then(|| wayfinder::file_model::query_file(&probe))
                            .flatten()
                    },
                    |w, found| {
                        if let Some((parent, data)) = found {
                            if !data.is_directory {
                                // Same as opening it from the list: the
                                // per-file app, or one chosen by name.
                                w.open_file(&FileObject::from_data(&parent, &data));
                            }
                        }
                    },
                );
            }

            window.present();
        }
    }

    fn shutdown(&self) {
        // (The session is written as windows change: see
        // WayfinderWindow::note_session_location.)
        self.parent_shutdown();
    }
}

impl GtkApplicationImpl for WayfinderApplicationInner {}
