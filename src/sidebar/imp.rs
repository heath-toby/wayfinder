use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

pub struct SidebarInner {
    pub places_list: gtk::ListBox,
    pub container: gtk::Box,
    /// Keep VolumeMonitor alive so its signals continue to fire.
    pub volume_monitor: gio::VolumeMonitor,
}

impl Default for SidebarInner {
    fn default() -> Self {
        let places_list = gtk::ListBox::new();
        places_list.set_selection_mode(gtk::SelectionMode::Single);
        places_list.add_css_class("navigation-sidebar");
        places_list.update_property(&[gtk::accessible::Property::Label("Places")]);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&places_list)
            .build();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_width_request(180);
        container.append(&scrolled);
        container.update_property(&[gtk::accessible::Property::Label("Sidebar")]);

        Self {
            places_list,
            container,
            volume_monitor: gio::VolumeMonitor::get(),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for SidebarInner {
    const NAME: &'static str = "WayfinderSidebar";
    type Type = super::WayfinderSidebar;
    type ParentType = glib::Object;
}

impl ObjectImpl for SidebarInner {}
