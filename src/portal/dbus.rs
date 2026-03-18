use std::collections::HashMap;

use zbus::zvariant::OwnedValue;
use zbus::interface;


pub struct FileChooserBackend {
    pub gtk_tx: std::sync::mpsc::Sender<ChooserRequest>,
}

#[allow(dead_code)]
pub struct ChooserRequest {
    pub handle: String,
    pub title: String,
    pub options: HashMap<String, OwnedValue>,
    pub is_save: bool,
    pub is_save_files: bool,
    pub response_tx: tokio::sync::oneshot::Sender<(u32, HashMap<String, OwnedValue>)>,
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooserBackend {
    async fn open_file(
        &self,
        handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
                handle: handle.to_string(),
                title: title.to_string(),
                options,
                is_save: false,
                is_save_files: false,
                response_tx: tx,
            })
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        rx.await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn save_file(
        &self,
        handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
                handle: handle.to_string(),
                title: title.to_string(),
                options,
                is_save: true,
                is_save_files: false,
                response_tx: tx,
            })
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        rx.await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn save_files(
        &self,
        handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
                handle: handle.to_string(),
                title: title.to_string(),
                options,
                is_save: true,
                is_save_files: true,
                response_tx: tx,
            })
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        rx.await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

pub fn parse_options(
    options: &HashMap<String, OwnedValue>,
) -> (bool, bool, Option<String>, Option<String>) {
    let multiple = options
        .get("multiple")
        .and_then(|v| <bool>::try_from(v.clone()).ok())
        .unwrap_or(false);

    let directory = options
        .get("directory")
        .and_then(|v| <bool>::try_from(v.clone()).ok())
        .unwrap_or(false);

    let current_folder = options.get("current_folder").and_then(|v| {
        let bytes: Result<Vec<u8>, _> = v.clone().try_into();
        bytes.ok().map(|b| {
            // Remove null terminator if present
            let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).to_string()
        })
    });

    let current_name = options
        .get("current_name")
        .and_then(|v| <String>::try_from(v.clone()).ok());

    (multiple, directory, current_folder, current_name)
}
