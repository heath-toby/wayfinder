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
        DbusRequest::Folders(uris) => {
            for uri in uris {
                let path = uri_to_path(&uri);
                open_or_raise(app, &path);
            }
        }
        DbusRequest::Items(uris) => {
            for uri in uris {
                show_item(app, &uri_to_path(&uri));
            }
        }
        DbusRequest::ItemProperties(uris) => {
            if let Some(uri) = uris.first() {
                show_item(app, &uri_to_path(uri));
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

/// Open the folder holding `path` with `path` selected ("Show in folder").
fn show_item(app: &gtk::Application, path: &str) {
    let path = PathBuf::from(path);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        open_or_raise(app, &path.to_string_lossy());
        return;
    };
    let parent = parent.to_string_lossy().to_string();
    let name = name.to_string_lossy().to_string();
    let window = app
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<crate::window::WayfinderWindow>().ok());
    let window = window.unwrap_or_else(|| crate::window::WayfinderWindow::new(app));
    window.remember_selection(&parent, &name);
    window.navigate_to_path(&parent);
    window.present();
}

fn open_or_raise(app: &gtk::Application, path: &str) {
    // Try to reuse an existing window
    for window in app.windows() {
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
