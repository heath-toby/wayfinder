//! Encrypted drives back after a suspend.
//!
//! Suspend (s2idle) switches USB and Thunderbolt devices off, and they come
//! back on wake as if plugged in again; an unlocked LUKS drive is cleaned
//! up (unmounted, locked) when it goes. As GNOME Shell does with a
//! passphrase in the keyring, a drive that vanished while mounted, and
//! whose passphrase the user chose to remember (`wayfinder::keyring`), is
//! unlocked and mounted again when it's back — so whatever waits for its
//! mount (a backup's .path unit, say) carries on. One the user unmounted is
//! left alone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use tokio::sync::mpsc::UnboundedSender;
use wayfinder::drives::{self, Watch};

use crate::Event;

/// How long after a mount goes the drive must still be there for it to
/// count as unmounted by the user.
const SETTLE: Duration = Duration::from_secs(3);

struct State {
    watch: Watch,
    /// Mounted encrypted drives: mount root → (LUKS UUID, drive).
    mounts: HashMap<String, (String, String)>,
    tx: UnboundedSender<Event>,
}

/// Watch for encrypted drives, on a thread of its own (GIO's volume
/// monitor needs a GLib main loop).
pub fn start(tx: UnboundedSender<Event>) {
    std::thread::spawn(move || {
        let ctx = glib::MainContext::new();
        let result = ctx.with_thread_default(|| {
            let monitor = gio::VolumeMonitor::get();
            let state = Rc::new(RefCell::new(State {
                watch: Watch::default(),
                mounts: HashMap::new(),
                tx,
            }));
            for m in monitor.mounts() {
                mount_added(&state, &m);
            }
            {
                let state = state.clone();
                monitor.connect_mount_added(move |_, m| mount_added(&state, m));
            }
            {
                let state = state.clone();
                monitor.connect_mount_removed(move |monitor, m| mount_removed(&state, monitor, m));
            }
            {
                let state = state.clone();
                monitor.connect_drive_disconnected(move |_, d| {
                    if let Some(id) = drive_id(d) {
                        log::info!("drive {id} disconnected");
                        state.borrow_mut().watch.drive_gone(&id);
                    }
                });
            }
            {
                let state = state.clone();
                monitor.connect_volume_added(move |_, v| volume_added(&state, v));
            }
            // (Kept alive by the loop below.)
            let _monitor = monitor;
            glib::MainLoop::new(Some(&glib::MainContext::ref_thread_default()), false).run();
        });
        if let Err(e) = result {
            log::warn!("can't watch encrypted drives: {e}");
        }
    });
}

fn drive_id(drive: &gio::Drive) -> Option<String> {
    drive
        .identifier(gio::DRIVE_IDENTIFIER_KIND_UNIX_DEVICE)
        .map(|s| s.to_string())
        .or_else(|| Some(drive.name().to_string()))
}

fn mount_root(m: &gio::Mount) -> Option<String> {
    m.root().path().map(|p| p.to_string_lossy().into_owned())
}

fn mount_added(state: &Rc<RefCell<State>>, m: &gio::Mount) {
    let Some(luks) = m
        .volume()
        .and_then(|v| v.identifier(gio::VOLUME_IDENTIFIER_KIND_UNIX_DEVICE))
        .and_then(|d| drives::luks_of_device(&d))
    else {
        return;
    };
    let (Some(root), Some(drive)) = (mount_root(m), m.drive().as_ref().and_then(drive_id)) else {
        return;
    };
    log::info!("watching encrypted drive at {root} (on {drive})");
    let mut s = state.borrow_mut();
    s.watch.mounted(&luks, &drive);
    s.mounts.insert(root, (luks, drive));
}

fn mount_removed(state: &Rc<RefCell<State>>, monitor: &gio::VolumeMonitor, m: &gio::Mount) {
    let Some(root) = mount_root(m) else { return };
    let Some((luks, drive)) = state.borrow_mut().mounts.remove(&root) else {
        return;
    };
    log::info!("encrypted drive at {root} unmounted or gone");
    state.borrow_mut().watch.unmounted(&luks);
    // Unmounted by the user, if its drive is still here a moment later.
    let state = state.clone();
    let monitor = monitor.clone();
    glib::timeout_add_local_once(SETTLE, move || {
        let present = monitor
            .connected_drives()
            .iter()
            .any(|d| drive_id(d).as_deref() == Some(drive.as_str()));
        if present {
            state.borrow_mut().watch.settle(&luks);
        }
    });
}

fn volume_added(state: &Rc<RefCell<State>>, vol: &gio::Volume) {
    let Some(uuid) = vol.uuid().map(|u| u.to_string()) else {
        return;
    };
    if !state.borrow().watch.is_awaited(&uuid) {
        return;
    }
    // Only with its passphrase remembered (the user's say-so): otherwise it
    // waits, locked, to be chosen in Wayfinder.
    let Some(pass) = wayfinder::keyring::recall(&uuid) else {
        state.borrow_mut().watch.returned(&uuid);
        return;
    };
    if !state.borrow_mut().watch.returned(&uuid) || !vol.can_mount() {
        return;
    }
    let name = vol.name().to_string();
    log::info!("{name} is back: unlocking it");
    let op = gio::MountOperation::new();
    let given = Rc::new(std::cell::Cell::new(false));
    op.connect_ask_password(move |op, _, _, _, _| {
        // Asked again: the remembered passphrase didn't work.
        if given.replace(true) {
            op.reply(gio::MountOperationResult::Aborted);
            return;
        }
        op.set_password(Some(&pass));
        op.set_password_save(gio::PasswordSave::Never);
        op.reply(gio::MountOperationResult::Handled);
    });
    let tx = state.borrow().tx.clone();
    let mounted = vol.clone();
    vol.mount(
        gio::MountMountFlags::NONE,
        Some(&op),
        gio::Cancellable::NONE,
        move |result| {
            let (path, message) = match result {
                Ok(()) => {
                    let path = mounted
                        .get_mount()
                        .and_then(|m| mount_root(&m))
                        .unwrap_or_default();
                    (path, format!("{name} is back: unlocked and mounted again."))
                }
                Err(e) => {
                    log::warn!("couldn't unlock {name} again: {e}");
                    let wrong = e.message().contains("passphrase")
                        || e.message().contains("Operation not permitted");
                    if wrong {
                        // Changed since: forgotten, so Wayfinder asks.
                        wayfinder::keyring::forget(&uuid);
                    }
                    (
                        String::new(),
                        format!("{name} is back, but couldn't be unlocked: {}", e.message()),
                    )
                }
            };
            log::info!("{message}");
            let _ = tx.send(Event::DriveBack(path, message));
        },
    );
}
