mod imp;

use gtk::gio;
use gtk::glib;

glib::wrapper! {
    pub struct FileObject(ObjectSubclass<imp::FileObject>);
}

/// Plain data extracted from a gio::FileInfo, usable across threads.
#[derive(Clone, PartialEq)]
pub struct FileInfoData {
    pub name: String,
    /// The name to show. In the Bin, an item trashed from another drive is
    /// named by its encoded trash path; this is its real name.
    pub display_name: String,
    /// Hidden as GIO sees it: a dot name, a backup (`name~`), or listed in
    /// the folder's `.hidden` file.
    pub is_hidden: bool,
    pub is_directory: bool,
    pub size: u64,
    pub modified: i64,
    pub modified_display: String,
    pub content_type: String,
}

impl FileInfoData {
    pub fn from_file_info(info: &gio::FileInfo) -> Self {
        let name = info.name().to_string_lossy().to_string();
        let file_type = info.file_type();
        let is_directory = file_type == gio::FileType::Directory;
        let size = if is_directory { 0 } else { info.size() as u64 };

        let modified = info
            .modification_date_time()
            .map(|dt| dt.to_unix())
            .unwrap_or(0);
        let modified_display = info
            .modification_date_time()
            .map(|dt| {
                // GIO gives UTC; show the time as it is here.
                dt.to_local()
                    .unwrap_or(dt)
                    .format("%Y-%m-%d %H:%M")
                    .unwrap_or_else(|_| glib::GString::from(""))
                    .to_string()
            })
            .unwrap_or_default();

        // FUSE listings ask only for the name-based guess (see
        // file_model::loader); asking for an attribute that wasn't
        // requested is a GLib critical.
        let content_type = if info.has_attribute(gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE) {
            info.content_type()
        } else {
            info.attribute_string(gio::FILE_ATTRIBUTE_STANDARD_FAST_CONTENT_TYPE)
        }
        .map(|s| s.to_string())
        .unwrap_or_default();

        let display_name = info.display_name().to_string();
        let is_hidden = name.starts_with('.')
            || (info.has_attribute(gio::FILE_ATTRIBUTE_STANDARD_IS_HIDDEN) && info.is_hidden());
        Self {
            name,
            display_name,
            is_hidden,
            is_directory,
            size,
            modified,
            modified_display,
            content_type,
        }
    }
}

impl FileObject {
    /// Build a FileObject from pre-extracted plain data (safe to call on the
    /// main thread after the data was gathered in a background thread).
    pub fn from_data(parent_path: &str, data: &FileInfoData) -> Self {
        let path = if parent_path.contains("://") {
            // In the Bin (trash:///): a URI, with the name escaped in it.
            use gio::prelude::FileExt;
            gio::File::for_uri(parent_path)
                .child(&data.name)
                .uri()
                .to_string()
        } else if parent_path == "/" {
            format!("/{}", data.name)
        } else {
            format!("{parent_path}/{}", data.name)
        };
        let hidden = data.is_hidden || data.name.starts_with('.');
        // Shown, sorted, searched and read out: the Bin's real names, not
        // its encoded ones. (The path keeps the name GIO knows it by.)
        let name = if parent_path.starts_with("trash:") && !data.display_name.is_empty() {
            &data.display_name
        } else {
            &data.name
        };
        let size_display = if data.is_directory {
            String::from("\u{2014}")
        } else {
            format_size(data.size)
        };
        let icon = if data.is_directory {
            String::from("folder")
        } else {
            gio::content_type_get_generic_icon_name(&data.content_type)
                .map(|s| s.to_string())
                .unwrap_or_else(|| String::from("text-x-generic"))
        };
        let file_type_name = if data.is_directory {
            String::from("Folder")
        } else if !data.content_type.is_empty() {
            gio::content_type_get_description(&data.content_type).to_string()
        } else {
            String::from("File")
        };
        let search_string = name.to_lowercase();
        let a11y_name = if data.is_directory {
            format!("{name}, Folder")
        } else {
            format!("{name}, {size_display}, {file_type_name}")
        };

        glib::Object::builder()
            .property("name", name)
            .property("path", &path)
            .property("icon", &icon)
            .property("size", data.size)
            .property("size-display", &size_display)
            .property("modified", data.modified)
            .property("modified-display", &data.modified_display)
            .property("mime-type", &data.content_type)
            .property("file-type-name", &file_type_name)
            .property("is-directory", data.is_directory)
            .property("hidden", hidden)
            .property("search-string", &search_string)
            .property("a11y-name", &a11y_name)
            .build()
    }

    /// Copy the details that can change (size, date, type) from `other`,
    /// an object for the same file, keeping this object and its place in
    /// any selection. Folder sizes are left alone: they're computed
    /// separately.
    pub fn update_from(&self, other: &FileObject) {
        if !self.is_directory() {
            self.set_size(other.size());
            self.set_size_display(other.size_display());
        }
        self.set_modified(other.modified());
        self.set_modified_display(other.modified_display());
        self.set_mime_type(other.mime_type());
        self.set_icon(other.icon());
        self.set_file_type_name(other.file_type_name());
        self.set_a11y_name(other.a11y_name());
    }

    pub fn from_file_info(parent_path: &str, info: &gio::FileInfo) -> Self {
        let name = info.name().to_string_lossy().to_string();

        let path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };

        let file_type = info.file_type();
        let is_directory = file_type == gio::FileType::Directory;
        let hidden = name.starts_with('.')
            || (info.has_attribute(gio::FILE_ATTRIBUTE_STANDARD_IS_HIDDEN) && info.is_hidden());

        let size = if is_directory { 0 } else { info.size() as u64 };
        let size_display = if is_directory {
            String::from("Calculating...")
        } else {
            format_size(size)
        };

        let modified = info
            .modification_date_time()
            .map(|dt| dt.to_unix())
            .unwrap_or(0);
        let modified_display = info
            .modification_date_time()
            .map(|dt| {
                // GIO gives UTC; show the time as it is here.
                dt.to_local()
                    .unwrap_or(dt)
                    .format("%Y-%m-%d %H:%M")
                    .unwrap_or_else(|_| glib::GString::from(""))
                    .to_string()
            })
            .unwrap_or_default();

        let content_type = info
            .content_type()
            .map(|s| s.to_string())
            .unwrap_or_default();

        let icon = if is_directory {
            String::from("folder")
        } else {
            gio::content_type_get_generic_icon_name(&content_type)
                .map(|s| s.to_string())
                .unwrap_or_else(|| String::from("text-x-generic"))
        };

        let file_type_name = if is_directory {
            String::from("Folder")
        } else if !content_type.is_empty() {
            gio::content_type_get_description(&content_type).to_string()
        } else {
            String::from("File")
        };

        let search_string = name.to_lowercase();

        let a11y_name = if is_directory {
            format!("{name}, Folder")
        } else {
            format!("{name}, {size_display}, {file_type_name}")
        };

        glib::Object::builder()
            .property("name", &name)
            .property("path", &path)
            .property("icon", &icon)
            .property("size", size)
            .property("size-display", &size_display)
            .property("modified", modified)
            .property("modified-display", &modified_display)
            .property("mime-type", &content_type)
            .property("file-type-name", &file_type_name)
            .property("is-directory", is_directory)
            .property("hidden", hidden)
            .property("search-string", &search_string)
            .property("a11y-name", &a11y_name)
            .build()
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
