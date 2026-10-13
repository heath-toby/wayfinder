// Wayfinder Portal — XDG FileChooser portal backend

mod chooser;
mod dbus;

use std::collections::HashMap;
use std::sync::mpsc;

use gtk::glib;
use zbus::zvariant::OwnedValue;

fn main() {
    env_logger::init();

    gtk::init().expect("Failed to init GTK");

    // Channel for D-Bus -> GTK communication
    let (gtk_tx, gtk_rx) = mpsc::channel::<dbus::ChooserRequest>();

    // Use a sync channel to wait for D-Bus name to be claimed before proceeding
    let (ready_tx, ready_rx) = mpsc::channel::<()>();

    // Start the D-Bus service in a background tokio runtime
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let backend = dbus::FileChooserBackend { gtk_tx };

            let conn = zbus::Connection::session().await.unwrap();
            conn.object_server()
                .at("/org/freedesktop/portal/desktop", backend)
                .await
                .unwrap();

            conn.request_name("org.freedesktop.impl.portal.desktop.wayfinder")
                .await
                .unwrap();

            log::info!("Wayfinder portal backend running on D-Bus");

            // Signal that D-Bus name is claimed
            let _ = ready_tx.send(());

            // Keep alive
            std::future::pending::<()>().await;
        });
    });

    // Wait for D-Bus name to be claimed (with timeout)
    match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(()) => log::info!("D-Bus name claimed, starting GTK main loop"),
        Err(_) => {
            log::error!("Timeout waiting for D-Bus name claim");
            std::process::exit(1);
        }
    }

    // Poll for D-Bus requests on the GTK main loop
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        while let Ok(request) = gtk_rx.try_recv() {
            log::info!("Received FileChooser request: {}", request.title);

            let (directory, current_folder, current_name) = dbus::parse_options(&request.options);

            let accept_label = request
                .options
                .get("accept_label")
                .and_then(|v| <String>::try_from(v.clone()).ok())
                .unwrap_or_else(|| {
                    if request.is_save {
                        "Save".to_string()
                    } else {
                        "Open".to_string()
                    }
                });

            let (result_tx, result_rx) = tokio::sync::oneshot::channel();

            // SaveFiles: the user picks a folder; each requested file is
            // saved there under its own name.
            let save_files = request
                .is_save_files
                .then(|| dbus::save_files_names(&request.options));

            chooser::show_chooser(
                &request.title,
                &accept_label,
                request.is_save && save_files.is_none(),
                directory || save_files.is_some(),
                current_folder.as_deref(),
                current_name.as_deref(),
                result_tx,
            );

            let response_tx = request.response_tx;
            glib::spawn_future_local(async move {
                if let Ok(mut result) = result_rx.await {
                    if let (Some(names), false) = (save_files, result.cancelled) {
                        // Checking for existing files may touch a slow mount.
                        let folder = result.uris.first().cloned().unwrap_or_default();
                        result.uris =
                            gtk::gio::spawn_blocking(move || save_files_uris(&folder, &names))
                                .await
                                .unwrap_or_default();
                    }
                    let response = if result.cancelled { 1u32 } else { 0u32 };
                    let mut results = HashMap::<String, OwnedValue>::new();
                    if !result.cancelled {
                        if let Ok(val) =
                            OwnedValue::try_from(zbus::zvariant::Value::from(result.uris))
                        {
                            results.insert("uris".to_string(), val);
                        }
                    }
                    let _ = response_tx.send((response, results));
                }
            });
        }
        glib::ControlFlow::Continue
    });

    // Run the GLib main loop
    let main_loop = glib::MainLoop::new(None, false);
    main_loop.run();
}

/// The URIs to save `names` under in the folder `folder_uri`: a name taken
/// by an existing file gets a number ("report (2).pdf"), so nothing is
/// overwritten without being asked.
fn save_files_uris(folder_uri: &str, names: &[String]) -> Vec<String> {
    use gtk::gio::prelude::FileExt;
    let folder = gtk::gio::File::for_uri(folder_uri);
    let Some(dir) = folder.path() else {
        return Vec::new();
    };
    let mut taken: Vec<std::path::PathBuf> = Vec::new();
    names
        .iter()
        // Only a name, in the folder the user chose: an app's
        // "../.config/…" or "/home/u/.profile" mustn't reach elsewhere.
        .filter_map(|name| plain_name(name))
        .map(|name| {
            let path = free_name(&dir, &name, |p| {
                p.exists() || taken.contains(&p.to_path_buf())
            });
            taken.push(path.clone());
            gtk::gio::File::for_path(&path).uri().to_string()
        })
        .collect()
}

/// The last part of `name`, if it's a usable file name (not empty, `.`
/// or `..`).
fn plain_name(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| n != "." && n != "..")
}

/// `dir/name`, or if `taken` says that's in use, `dir/stem (N).ext`.
fn free_name(
    dir: &std::path::Path,
    name: &str,
    taken: impl Fn(&std::path::Path) -> bool,
) -> std::path::PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::{free_name, plain_name};

    /// An app can't save outside the folder chosen.
    #[test]
    fn app_names_stay_in_the_folder() {
        assert_eq!(
            plain_name("../.config/autostart/x.desktop").as_deref(),
            Some("x.desktop")
        );
        assert_eq!(plain_name("/home/u/.profile").as_deref(), Some(".profile"));
        assert_eq!(plain_name(".."), None);
        assert_eq!(plain_name("a.txt").as_deref(), Some("a.txt"));
    }
    use std::path::Path;

    #[test]
    fn taken_names_get_a_number() {
        let dir = Path::new("/d");
        let taken =
            |p: &Path| p == Path::new("/d/report.pdf") || p == Path::new("/d/report (2).pdf");
        assert_eq!(
            free_name(dir, "report.pdf", taken),
            Path::new("/d/report (3).pdf")
        );
        assert_eq!(
            free_name(dir, "notes", |p: &Path| p == Path::new("/d/notes")),
            Path::new("/d/notes (2)")
        );
        assert_eq!(
            free_name(dir, "new.txt", |_: &Path| false),
            Path::new("/d/new.txt")
        );
    }
}
