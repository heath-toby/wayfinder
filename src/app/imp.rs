use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::window::WayfinderWindow;
use wayfinder::file_object::FileObject;

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
