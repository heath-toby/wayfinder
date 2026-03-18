// Wayfinder Portal — XDG FileChooser portal backend

mod chooser;
mod dbus;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;

use gtk::glib;
use gtk::prelude::*;
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

            let (_multiple, directory, current_folder, current_name) =
                dbus::parse_options(&request.options);

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

            chooser::show_chooser(
                &request.title,
                &accept_label,
                request.is_save,
                false,
                directory,
                current_folder.as_deref(),
                current_name.as_deref(),
                vec![],
                result_tx,
            );

            let response_tx = request.response_tx;
            glib::spawn_future_local(async move {
                if let Ok(result) = result_rx.await {
                    let response = if result.cancelled { 1u32 } else { 0u32 };
                    let mut results = HashMap::<String, OwnedValue>::new();
                    if !result.cancelled {
                        results.insert(
                            "uris".to_string(),
                            OwnedValue::try_from(zbus::zvariant::Value::from(result.uris))
                                .unwrap(),
                        );
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
