use std::path::PathBuf;
use std::sync::mpsc;

use gtk::glib;
use gtk::prelude::*;

/// Messages from the D-Bus service to the GTK main thread
pub enum DbusRequest {
    Folders(Vec<String>),
    Items(Vec<String>),
    ItemProperties(Vec<String>),
}

/// Start the D-Bus FileManager1 service in a background thread.
/// Returns a receiver for incoming requests that should be polled on the GTK main thread.
pub fn start_service() -> mpsc::Receiver<DbusRequest> {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for D-Bus");

        rt.block_on(async move {
            if let Err(e) = run_service(tx).await {
                log::warn!("D-Bus FileManager1 service failed: {e}");
            }
        });
    });

    rx
}

async fn run_service(tx: mpsc::Sender<DbusRequest>) -> Result<(), zbus::Error> {
    let fm = FileManager1 { tx };

    let _conn = zbus::connection::Builder::session()?
        .name("org.freedesktop.FileManager1")?
        .serve_at("/org/freedesktop/FileManager1", fm)?
        .build()
        .await?;

    log::info!("D-Bus FileManager1 service registered");

    // Keep the connection alive
    std::future::pending::<()>().await;

    Ok(())
}

struct FileManager1 {
    tx: mpsc::Sender<DbusRequest>,
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1 {
    /// Open folders in the file manager
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.tx.send(DbusRequest::Folders(uris));
    }

    /// Show items (navigate to parent and select)
    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.tx.send(DbusRequest::Items(uris));
    }

    /// Show the properties dialog for items
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.tx.send(DbusRequest::ItemProperties(uris));
    }
}

/// Set up polling for D-Bus requests on the GTK main thread.
/// Call this after the application has started.
pub fn connect_to_app(app: &gtk::Application, rx: mpsc::Receiver<DbusRequest>) {
    let app = app.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(request) = rx.try_recv() {
            handle_request(&app, request);
        }
        glib::ControlFlow::Continue
    });
}

fn handle_request(app: &gtk::Application, request: DbusRequest) {
    match request {
        // One window each: in one window, each would replace the last.
        DbusRequest::Folders(uris) => {
            for (i, uri) in uris.iter().enumerate() {
                open_or_raise(app, &uri_to_path(uri), i > 0);
            }
        }
        DbusRequest::Items(uris) => {
            // One window per folder: items in the same one share it.
            let paths: Vec<String> = uris.iter().map(|u| uri_to_path(u)).collect();
            let mut folders: Vec<Option<PathBuf>> = Vec::new();
            for path in paths {
                let folder = PathBuf::from(&path).parent().map(PathBuf::from);
                if folders.contains(&folder) {
                    continue;
                }
                let fresh = !folders.is_empty();
                folders.push(folder);
                show_item(app, &path, fresh, false);
            }
        }
        DbusRequest::ItemProperties(uris) => {
            if let Some(uri) = uris.first() {
                show_item(app, &uri_to_path(uri), false, true);
            }
        }
    }
}

/// A `file://` URI as a path, percent-decoded (`My%20Files` is
/// `My Files`); anything else as it is.
fn uri_to_path(uri: &str) -> String {
    if uri.starts_with("file://") {
        if let Some(path) = gtk::gio::File::for_uri(uri).path() {
            return path.to_string_lossy().to_string();
        }
    }
    uri.to_string()
}

/// Open the folder holding `path` with `path` selected ("Show in folder"),
/// in a new window if `fresh`; then show its properties, if `properties`.
fn show_item(app: &gtk::Application, path: &str, fresh: bool, properties: bool) {
    let path = PathBuf::from(path);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        open_or_raise(app, &path.to_string_lossy(), fresh);
        return;
    };
    let parent = parent.to_string_lossy().to_string();
    let name = name.to_string_lossy().to_string();
    let window = app
        .windows()
        .into_iter()
        .filter(|_| !fresh)
        .find_map(|w| w.downcast::<crate::window::WayfinderWindow>().ok());
    let window = window.unwrap_or_else(|| crate::window::WayfinderWindow::new(app));
    window.remember_selection(&parent, &name);
    if properties {
        window.show_properties_when_opened(&parent, &name);
    }
    window.navigate_to_path(&parent);
    window.present();
}

fn open_or_raise(app: &gtk::Application, path: &str, fresh: bool) {
    // Try to reuse an existing window
    for window in app.windows().into_iter().filter(|_| !fresh) {
        if let Some(wf_window) = window.downcast_ref::<crate::window::WayfinderWindow>() {
            wf_window.navigate_to_path(path);
            wf_window.present();
            return;
        }
    }

    // No existing window — create one
    let window = crate::window::WayfinderWindow::new(app);
    window.navigate_to_path(path);
    window.present();
}
