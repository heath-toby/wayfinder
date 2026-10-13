use std::collections::HashMap;

use zbus::interface;
use zbus::zvariant::OwnedValue;

pub struct FileChooserBackend {
    pub gtk_tx: std::sync::mpsc::Sender<ChooserRequest>,
}

pub struct ChooserRequest {
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
        _handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
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
        _handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
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
        _handle: &str,
        _app_id: &str,
        _parent_window: &str,
        title: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.gtk_tx
            .send(ChooserRequest {
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

/// (directory, current_folder, current_name). Multiple selection isn't
/// offered, so `multiple` isn't read.
pub fn parse_options(
    options: &HashMap<String, OwnedValue>,
) -> (bool, Option<String>, Option<String>) {
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

    (directory, current_folder, current_name)
}

/// The file names a SaveFiles request wants saved (`files`, an array of
/// byte strings, maybe NUL-terminated).
pub fn save_files_names(options: &HashMap<String, OwnedValue>) -> Vec<String> {
    options
        .get("files")
        .and_then(|v| <Vec<Vec<u8>>>::try_from(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|b| {
            let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).to_string()
        })
        .filter(|n| !n.is_empty())
        .collect()
}
