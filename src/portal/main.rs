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
            // Asked to save no files (a request not understood): failed, at
            // once — not after the user has chosen a folder.
            if save_files.as_ref().is_some_and(|names| names.is_empty()) {
                log::warn!("SaveFiles request with no file names");
                let _ = request.response_tx.send((2, HashMap::new()));
                continue;
            }

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
                    let mut failed = false;
                    if let (Some(names), false) = (save_files, result.cancelled) {
                        // Checking for existing files may touch a slow mount.
                        let folder = result.uris.first().cloned().unwrap_or_default();
                        let uris =
                            gtk::gio::spawn_blocking(move || save_files_uris(&folder, &names))
                                .await
                                .unwrap_or_else(|_| {
                                    Err("The names couldn't be checked. Try again.".to_string())
                                });
                        match uris {
                            Ok(uris) => result.uris = uris,
                            // No safe names: the app is told it failed, and
                            // the user (the chooser has closed) by an alert,
                            // saying why.
                            Err(why) => {
                                failed = true;
                                gtk::AlertDialog::builder()
                                    .message("The files weren't saved")
                                    .detail(format!("{why}."))
                                    .build()
                                    .show(None::<&gtk::Window>);
                            }
                        }
                    }
                    let response = if failed {
                        2u32
                    } else if result.cancelled {
                        1u32
                    } else {
                        0u32
                    };
                    let mut results = HashMap::<String, OwnedValue>::new();
                    if !result.cancelled && !failed {
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
/// An error, saying why, if a name that's surely free can't be found for
/// every file: the save fails rather than risk one.
fn save_files_uris(folder_uri: &str, names: &[String]) -> Result<Vec<String>, String> {
    use gtk::gio::prelude::FileExt;
    // (No names asked for — a request not understood — is no save either.)
    if names.is_empty() {
        return Err("No files were asked to be saved".to_string());
    }
    if folder_uri.is_empty() {
        return Err("No folder was chosen".to_string());
    }
    let folder = gtk::gio::File::for_uri(folder_uri);
    let dir = folder
        .path()
        .ok_or_else(|| "The folder chosen isn't one on this computer".to_string())?;
    let mut taken: Vec<String> = Vec::new();
    names
        .iter()
        // Only a name, in the folder the user chose: an app's
        // "../.config/…" or "/home/u/.profile" mustn't reach elsewhere.
        // (One that's no name at all gets a stand-in: the list must stay in
        // step with the app's.)
        .map(|name| plain_name(name).unwrap_or_else(|| "file".to_string()))
        .map(|name| {
            // (Taken: anything there, a link leading nowhere included — and
            // a name given to an earlier file, in any case: some drives
            // take Report and report for one.) Only "not found" is free; a
            // name that can't be checked stops it, saying why.
            let error = std::cell::RefCell::new(None);
            let path = free_name(&dir, &name, |p| {
                let free = match wayfinder::file_ops::name_free(p) {
                    Ok(free) => free,
                    Err(e) => {
                        // (In plain words: the kind, not the OS's code.)
                        *error.borrow_mut() = Some(e.kind().to_string());
                        return None;
                    }
                };
                let lower = p.to_string_lossy().to_lowercase();
                Some(!free || taken.contains(&lower))
            })
            // (Not naming the file: the name is the app's, and its words
            // mustn't speak in this window.)
            .ok_or_else(|| match error.take() {
                Some(e) => format!(
                    "A name couldn't be checked in the folder chosen: {e}. Try another folder"
                ),
                None => "Every name for one of the files is taken in the folder chosen. Try another folder"
                    .to_string(),
            })?;
            taken.push(path.to_string_lossy().to_lowercase());
            Ok(gtk::gio::File::for_path(&path).uri().to_string())
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
/// `taken`: whether a path is taken; `None` if that can't be told — then
/// there's no name (stopping at once: a mount timing out would make each
/// try wait).
fn free_name(
    dir: &std::path::Path,
    name: &str,
    taken: impl Fn(&std::path::Path) -> Option<bool>,
) -> Option<std::path::PathBuf> {
    let first = dir.join(name);
    if !taken(&first)? {
        return Some(first);
    }
    // (Not forever: if every number is "taken" — the folder can't be looked
    // in, or holds them all — numbers from the time, checked as any; and if
    // none is free, none: never a name found taken, nor one unchecked.)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let base = u32::try_from(now % 1_000_000_000).unwrap_or(0) + 1000;
    for n in (2..1000).chain(base..base + 3) {
        let path = dir.join(wayfinder::file_ops::numbered_name(name, n));
        if !taken(&path)? {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn saving_no_files_or_names_differing_in_case() {
        assert!(save_files_uris("file:///tmp", &[]).is_err());
        let d = std::env::temp_dir().join(format!("wayfinder-savefiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let uri = gtk::gio::prelude::FileExt::uri(&gtk::gio::File::for_path(&d)).to_string();
        let got = save_files_uris(&uri, &["Report.pdf".into(), "report.pdf".into()]).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        // Not one file for both, even on a drive that ignores case.
        assert_ne!(got[0].to_lowercase(), got[1].to_lowercase());
    }

    use super::{free_name, plain_name, save_files_uris};

    #[test]
    fn a_folder_with_every_number_taken_still_gets_a_new_name() {
        let dir = std::path::Path::new("/d");
        let numbered = |p: &std::path::Path| {
            let name = p.file_name().unwrap().to_string_lossy();
            name == "scan.pdf"
                || name.strip_prefix("scan (").is_some_and(|r| {
                    r.trim_end_matches(").pdf")
                        .parse::<u32>()
                        .is_ok_and(|n| n < 1000)
                })
        };
        let got = free_name(dir, "scan.pdf", |p| Some(numbered(p))).unwrap();
        assert!(!numbered(&got), "{got:?}");
        // Every name taken: none. Nor if it can't be told (at once).
        assert_eq!(free_name(dir, "scan.pdf", |_| Some(true)), None);
        let tries = std::cell::Cell::new(0);
        let none = free_name(dir, "scan.pdf", |_| {
            tries.set(tries.get() + 1);
            None
        });
        assert_eq!((none, tries.get()), (None, 1));
    }

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
            |p: &Path| Some(p == Path::new("/d/report.pdf") || p == Path::new("/d/report (2).pdf"));
        assert_eq!(
            free_name(dir, "report.pdf", taken),
            Some(Path::new("/d/report (3).pdf").to_path_buf())
        );
        assert_eq!(
            free_name(dir, "notes", |p: &Path| Some(p == Path::new("/d/notes"))),
            Some(Path::new("/d/notes (2)").to_path_buf())
        );
        assert_eq!(
            free_name(dir, "new.txt", |_: &Path| Some(false)),
            Some(Path::new("/d/new.txt").to_path_buf())
        );
    }
}
