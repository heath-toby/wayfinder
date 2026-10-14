use std::cell::RefCell;

use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

#[derive(Properties, Default)]
#[properties(wrapper_type = super::FileObject)]
pub struct FileObject {
    #[property(get, set)]
    name: RefCell<String>,
    #[property(get, set)]
    path: RefCell<String>,
    #[property(get, set)]
    icon: RefCell<String>,
    #[property(get, set)]
    size: RefCell<u64>,
    #[property(get, set)]
    size_display: RefCell<String>,
    #[property(get, set)]
    modified: RefCell<i64>,
    #[property(get, set)]
    modified_display: RefCell<String>,
    #[property(get, set)]
    mime_type: RefCell<String>,
    #[property(get, set)]
    file_type_name: RefCell<String>,
    #[property(get, set)]
    is_directory: RefCell<bool>,
    #[property(get, set)]
    hidden: RefCell<bool>,
    #[property(get, set)]
    search_string: RefCell<String>,
    #[property(get, set)]
    a11y_name: RefCell<String>,
    /// In the Bin: the folder it was deleted from (empty elsewhere).
    #[property(get, set)]
    original_location: RefCell<String>,
}

#[glib::object_subclass]
impl ObjectSubclass for FileObject {
    const NAME: &'static str = "WayfinderFileObject";
    type Type = super::FileObject;
    type ParentType = glib::Object;
}

#[glib::derived_properties]
impl ObjectImpl for FileObject {}
