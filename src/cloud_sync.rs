//! Wayfinder's side of Wayfinder Sync (the `wayfinder-sync` service, see
//! `wayfinder::cloud`): the setup guide, asking it to sync, and announcing
//! what it reports.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use crate::window::WayfinderWindow;

const SERVICE: &str = "org.wayfinder.Sync";
const OBJECT_PATH: &str = "/org/wayfinder/Sync";
const UNIT: &str = "wayfinder-sync.service";

/// Whether `path` is on a drive Wayfinder Sync has been set up for.
pub fn on_sync_drive(path: &str) -> bool {
    wayfinder::cloud::drive_for(
        &wayfinder::offline::real_location(path),
        &wayfinder::cloud::load_drives(),
    )
    .is_some()
}

fn call(method: &str, path: &str) {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };
    conn.call(
        Some(SERVICE),
        OBJECT_PATH,
        SERVICE,
        method,
        Some(&(path,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
        |_| {},
    );
}

/// What to open for the file at `path`: its offline copy, if it's kept
/// offline on a drive Wayfinder Sync keeps in step and the copy is all
/// there — instant, no streaming through the mount, and what the app
/// saves is sent to the drive by the service. Otherwise `path` itself.
/// Opening it also has the item checked, so a change made elsewhere comes
/// down promptly.
pub fn file_to_open(path: &str) -> String {
    if !on_sync_drive(path) {
        return path.to_string();
    }
    match wayfinder::offline::copy_to_open(path) {
        Some(copy) => {
            check_now(path);
            copy.to_string_lossy().into_owned()
        }
        None => path.to_string(),
    }
}

/// Sync the item holding `path` now, and hear back even if nothing changed.
pub fn sync_now(path: &str) {
    call("SyncNow", path);
}

/// Have the item holding `path` checked now, quietly unless something
/// changed (a synced folder being opened).
pub fn check_now(path: &str) {
    call("CheckNow", path);
}

/// Listen for the service's results: announce them, and refresh windows
/// showing what was synced.
pub fn listen(app: &gtk::Application) {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };
    let app = app.downgrade();
    #[allow(deprecated)]
    let id = conn.signal_subscribe(
        Some(SERVICE),
        Some(SERVICE),
        Some("Synced"),
        Some(OBJECT_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, params| {
            let Some(app) = app.upgrade() else {
                return;
            };
            let Some((item, message)) = params.get::<(String, String)>() else {
                return;
            };
            let windows: Vec<WayfinderWindow> = app
                .windows()
                .into_iter()
                .filter_map(|w| w.downcast::<WayfinderWindow>().ok())
                .collect();
            for w in &windows {
                let shown = wayfinder::offline::real_location(&w.imp().model.current_path());
                if shown == item || shown.starts_with(&format!("{item}/")) {
                    w.reload_current();
                }
            }
            if !message.is_empty() {
                let target = app
                    .active_window()
                    .and_downcast::<WayfinderWindow>()
                    .or_else(|| windows.first().cloned());
                if let Some(w) = target {
                    w.announce(&message, AccessibleAnnouncementPriority::Medium);
                }
            }
        },
    );
    // The subscription lasts as long as the connection: keep that for the
    // life of the app (GIO's shared bus is freed with its last reference).
    let _ = id;
    std::mem::forget(conn);
}

/// The drive's remote top level and mountpoint to offer for `path`: from
/// its running rclone mount, else from the drives Wayfinder has seen
/// mounted and the first configured remote.
fn guess(path: &str) -> (String, String) {
    if let Some((mountpoint, remote)) = wayfinder::rclone_ops::mount_of(path) {
        // (A mount of a remote's subfolder: the remote's top level.)
        let top = remote
            .split_once(':')
            .map(|(name, _)| format!("{name}:"))
            .unwrap_or(remote);
        return (top, mountpoint);
    }
    let mountpoint = wayfinder::offline::known_mountpoints()
        .into_iter()
        .filter(|m| path == m || path.starts_with(&format!("{m}/")))
        .max_by_key(|m| m.len())
        .unwrap_or_default();
    let remote = wayfinder::rclone_ops::list_remotes()
        .into_iter()
        .next()
        .unwrap_or_default();
    (remote, mountpoint)
}

/// Offer to set up Wayfinder Sync for the drive `path` is on, the first
/// time something there is kept offline. `then(true)` once it's set up,
/// `then(false)` if the user chose not now (the item is then kept offline
/// one way, as before).
pub fn setup_guide(window: &WayfinderWindow, path: &str, then: impl FnOnce(bool) + 'static) {
    let (remote, mountpoint) = guess(path);
    let dlg = gtk::Window::builder()
        .title("Set Up Wayfinder Sync")
        .modal(true)
        .transient_for(window)
        .default_width(520)
        .build();
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    vbox.set_margin_top(14);
    vbox.set_margin_bottom(14);
    vbox.set_margin_start(14);
    vbox.set_margin_end(14);
    let intro = gtk::Label::builder()
        .label(
            "Wayfinder Sync keeps what you keep offline in step with your cloud drive, \
             both ways: changes you make are sent up a few seconds after you save, and \
             changes made elsewhere come down within a few minutes, even while Wayfinder \
             is closed. Check the details below, then choose Set Up.",
        )
        .wrap(true)
        .xalign(0.0)
        .build();
    vbox.append(&intro);

    let remote_label = gtk::Label::builder()
        .label("Cloud drive (the rclone remote's top level, such as Proton:)")
        .xalign(0.0)
        .build();
    let remote_entry = gtk::Entry::builder().text(&remote).build();
    remote_entry.update_property(&[gtk::accessible::Property::Label(
        "Cloud drive: the rclone remote's top level, such as Proton:",
    )]);
    remote_label.set_mnemonic_widget(Some(&remote_entry));
    vbox.append(&remote_label);
    vbox.append(&remote_entry);
    let remotes = wayfinder::rclone_ops::list_remotes();
    if !remotes.is_empty() {
        let hint = gtk::Label::builder()
            .label(format!("Your rclone remotes: {}", remotes.join(", ")))
            .xalign(0.0)
            .wrap(true)
            .build();
        vbox.append(&hint);
    }

    let mount_label = gtk::Label::builder()
        .label("Mounted at")
        .xalign(0.0)
        .build();
    let mount_entry = gtk::Entry::builder().text(&mountpoint).build();
    mount_entry.update_property(&[gtk::accessible::Property::Label("Mounted at")]);
    mount_label.set_mnemonic_widget(Some(&mount_entry));
    vbox.append(&mount_label);
    vbox.append(&mount_entry);

    let manage = gtk::CheckButton::with_label(
        "Let Wayfinder Sync run this drive's mount (recommended): it mounts the drive when you log in, and again if it drops",
    );
    manage.set_active(true);
    vbox.append(&manage);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let not_now = gtk::Button::with_label("Not Now");
    let set_up = gtk::Button::with_label("Set Up");
    set_up.add_css_class("suggested-action");
    buttons.append(&not_now);
    buttons.append(&set_up);
    vbox.append(&buttons);
    dlg.set_child(Some(&vbox));
    dlg.set_default_widget(Some(&set_up));

    let then = std::rc::Rc::new(std::cell::RefCell::new(Some(Box::new(then) as Box<dyn FnOnce(bool)>)));
    let finish = {
        let then = then.clone();
        move |ok: bool| {
            if let Some(cb) = then.borrow_mut().take() {
                cb(ok);
            }
        }
    };
    {
        let dlg = dlg.clone();
        let finish = finish.clone();
        not_now.connect_clicked(move |_| {
            dlg.close();
            finish(false);
        });
    }
    {
        let finish = finish.clone();
        dlg.connect_close_request(move |_| {
            finish(false);
            glib::Propagation::Proceed
        });
    }
    {
        let dlg = dlg.clone();
        let w = window.clone();
        set_up.connect_clicked(move |_| {
            let remote = remote_entry.text().trim().to_string();
            let mountpoint = mount_entry.text().trim().trim_end_matches('/').to_string();
            if !remote.contains(':') {
                w.announce(
                    "The cloud drive must be an rclone remote's name followed by a colon, such as Proton:",
                    AccessibleAnnouncementPriority::High,
                );
                remote_entry.grab_focus();
                return;
            }
            if !mountpoint.starts_with('/') {
                w.announce(
                    "Mounted at must be a full path, such as /home/you/protonDrive",
                    AccessibleAnnouncementPriority::High,
                );
                mount_entry.grab_focus();
                return;
            }
            let manage_mount = manage.is_active();
            let w2 = w.clone();
            let finish = finish.clone();
            dlg.close();
            w.run_blocking(
                "/",
                move || set_up_drive(&remote, &mountpoint, manage_mount),
                move |_, result| match result {
                    Ok(msg) => {
                        w2.announce(&msg, AccessibleAnnouncementPriority::Medium);
                        finish(true);
                    }
                    Err(e) => {
                        w2.announce(
                            &format!("Couldn't set up Wayfinder Sync: {e}"),
                            AccessibleAnnouncementPriority::High,
                        );
                        finish(false);
                    }
                },
            );
        });
    }
    dlg.present();
}

/// Record the drive, install and start the service. Blocking (systemctl):
/// off the main thread. Returns what to tell the user.
fn set_up_drive(remote: &str, mountpoint: &str, manage_mount: bool) -> Result<String, String> {
    let mut drives = wayfinder::cloud::load_drives();
    drives.retain(|d| d.mountpoint != mountpoint);
    let port = (5572..5672)
        .find(|p| !drives.iter().any(|d| d.rc_port == *p))
        .unwrap_or(5572);
    drives.push(wayfinder::cloud::Drive {
        remote: remote.to_string(),
        mountpoint: mountpoint.to_string(),
        manage_mount,
        rc_port: port,
    });
    wayfinder::cloud::save_drives(&drives)?;
    install_unit()?;
    let systemctl = |args: &[&str]| -> Result<(), String> {
        let ok = std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success();
        if ok {
            Ok(())
        } else {
            Err(format!("systemctl --user {} failed", args.join(" ")))
        }
    };
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", UNIT])?;
    // (Running already, with another drive: it reads the drives file as it
    // goes, but start it again so a new mount is picked up at once.)
    systemctl(&["restart", UNIT])?;
    let name = remote.trim_end_matches(':');
    let mut msg = format!("Wayfinder Sync is set up for {name}.");
    let mounted_by_hand = manage_mount
        && wayfinder::rclone_ops::mount_of(mountpoint).is_some_and(|(m, _)| m == mountpoint);
    if mounted_by_hand {
        msg.push_str(
            " The drive's current mount stays in use; Wayfinder Sync takes the mount over once it's unmounted, or from your next login.",
        );
    }
    Ok(msg)
}

/// Write `~/.config/systemd/user/wayfinder-sync.service`, pointing at the
/// `wayfinder-sync` installed beside this program.
fn install_unit() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let service = exe
        .parent()
        .map(|d| d.join("wayfinder-sync"))
        .filter(|p| p.exists())
        .ok_or("wayfinder-sync isn't installed beside wayfinder")?;
    let dir = dirs::config_dir()
        .ok_or("no config folder")?
        .join("systemd/user");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let unit = format!(
        "[Unit]\n\
         Description=Wayfinder Sync: offline folders kept in step with cloud drives\n\
         \n\
         [Service]\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        service.display()
    );
    std::fs::write(dir.join(UNIT), unit).map_err(|e| e.to_string())
}
