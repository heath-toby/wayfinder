use std::cell::RefCell;

use gtk::gio;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOperation {
    Copy,
    Cut,
}

#[derive(Clone)]
pub struct ClipboardState {
    pub operation: ClipboardOperation,
    pub files: Vec<gio::File>,
}

impl ClipboardState {
    pub fn new(operation: ClipboardOperation, files: Vec<gio::File>) -> Self {
        Self { operation, files }
    }
}

// Global (app-wide) clipboard shared across all windows.
// Safe because GTK is single-threaded.
thread_local! {
    static GLOBAL_CLIPBOARD: RefCell<Option<ClipboardState>> = const { RefCell::new(None) };
}

pub fn global_set(state: ClipboardState) {
    GLOBAL_CLIPBOARD.with(|c| *c.borrow_mut() = Some(state));
}

pub fn global_get() -> Option<ClipboardState> {
    GLOBAL_CLIPBOARD.with(|c| c.borrow().clone())
}

pub fn global_clear() {
    GLOBAL_CLIPBOARD.with(|c| *c.borrow_mut() = None);
}

use gtk::{gdk, glib, prelude::*};

const GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";
const KDE_CUT_SELECTION: &str = "application/x-kde-cutselection";

/// Put files on the system clipboard in the formats Nautilus and Dolphin use,
/// so other apps can take them: Signal and WhatsApp attach them, and other file
/// managers can paste them. The `gdk::FileList` value becomes `text/uri-list`
/// and the portal file-transfer formats (for Flatpak apps). The GNOME and KDE
/// formats say whether the files were copied or cut.
///
/// On Wayland this must run while one of our surfaces has keyboard focus,
/// otherwise the compositor silently ignores it.
pub fn publish(display: &gdk::Display, state: &ClipboardState) {
    watch_ownership(display);
    let file_list = gdk::FileList::from_array(&state.files);
    let verb = match state.operation {
        ClipboardOperation::Copy => "copy",
        ClipboardOperation::Cut => "cut",
    };
    let mut gnome = String::from(verb);
    for f in &state.files {
        gnome.push('\n');
        gnome.push_str(&f.uri());
    }
    let kde_cut = if state.operation == ClipboardOperation::Cut { "1" } else { "0" };
    let providers = [
        gdk::ContentProvider::for_value(&file_list.to_value()),
        gdk::ContentProvider::for_bytes(GNOME_COPIED_FILES, &glib::Bytes::from_owned(gnome.into_bytes())),
        gdk::ContentProvider::for_bytes(KDE_CUT_SELECTION, &glib::Bytes::from_static(kde_cut.as_bytes())),
    ];
    if let Err(e) = display
        .clipboard()
        .set_content(Some(&gdk::ContentProvider::new_union(&providers)))
    {
        log::warn!("Failed to publish files to the system clipboard: {e}");
    }
}

/// Read files another application put on the system clipboard. Returns `None`
/// if Wayfinder itself owns the clipboard, because then the internal state is
/// the authoritative copy. Also returns `None` if the clipboard holds no files.
pub async fn read_system(display: &gdk::Display) -> Option<ClipboardState> {
    let clipboard = display.clipboard();
    if clipboard.is_local() {
        return None;
    }
    let formats = clipboard.formats();

    if formats.contain_mime_type(GNOME_COPIED_FILES) {
        if let Some(text) = read_mime_text(&clipboard, GNOME_COPIED_FILES).await {
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            let operation = match lines.next() {
                Some("cut") => ClipboardOperation::Cut,
                _ => ClipboardOperation::Copy,
            };
            let files: Vec<gio::File> = lines.map(gio::File::for_uri).collect();
            if !files.is_empty() {
                return Some(ClipboardState::new(operation, files));
            }
        }
    }

    if formats.contains_type(gdk::FileList::static_type()) {
        let value = clipboard
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await
            .ok()?;
        let files = value.get::<gdk::FileList>().ok()?.files();
        if files.is_empty() {
            return None;
        }
        let cut = formats.contain_mime_type(KDE_CUT_SELECTION)
            && read_mime_text(&clipboard, KDE_CUT_SELECTION)
                .await
                .is_some_and(|t| t.trim() == "1");
        let operation = if cut { ClipboardOperation::Cut } else { ClipboardOperation::Copy };
        return Some(ClipboardState::new(operation, files));
    }

    None
}

async fn read_mime_text(clipboard: &gdk::Clipboard, mime: &str) -> Option<String> {
    let (stream, _) = clipboard
        .read_future(&[mime], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let mut data = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .ok()?;
        if chunk.is_empty() {
            break;
        }
        data.extend_from_slice(&chunk);
    }
    String::from_utf8(data).ok()
}

/// When another app takes over the system clipboard, forget the app-wide
/// files too, so Paste doesn't bring back old files after you've copied
/// something else. Connected once, the first time we publish.
fn watch_ownership(display: &gdk::Display) {
    thread_local! {
        static WATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    display.clipboard().connect_changed(|clipboard| {
        if !clipboard.is_local() {
            global_clear();
        }
    });
}
