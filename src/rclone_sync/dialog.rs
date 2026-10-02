//! Sync configuration dialog and progress display.

use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use crate::rclone_ops::{list_remotes, rclone_available};

use super::{pair_for, remove_pair, set_pair};

/// Show a dialog to configure two-way sync for the given local path.
///
/// If a sync pair already exists, allows editing or removing it. Otherwise
/// creates a new pair and offers to run the initial sync immediately.
pub fn show_dialog(window: &impl IsA<gtk::Window>, local_path: &str, on_close: impl Fn() + 'static) {
    let parent: gtk::Window = window.clone().upcast();

    if !rclone_available() {
        parent.announce(
            "rclone is not installed. Install it with your package manager and try again.",
            AccessibleAnnouncementPriority::High,
        );
        on_close();
        return;
    }

    let existing = pair_for(local_path);
    let editing = existing.is_some();
    let title = if editing {
        "Edit Sync"
    } else {
        "Set Up Two-Way Sync"
    };

    let dlg = gtk::Window::builder()
        .title(title)
        .modal(true)
        .transient_for(&parent)
        .default_width(500)
        .default_height(360)
        .build();
    dlg.update_property(&[gtk::accessible::Property::Label(title)]);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    // If the user is setting up sync on a folder inside an rclone FUSE mount,
    // we redirect bisync to the offline cache path. Bisyncing the FUSE
    // mount against the same remote it's mounted from would be syncing the
    // remote with itself — the disaster scenario.
    //
    // We trust the kernel (`/proc/mounts` fstype `fuse.rclone`) for the
    // is-this-rclone check, then fall back to ps-based parsing for the
    // remote-spec lookup. If the kernel says "rclone FUSE" but the parser
    // can't extract the remote, we still redirect to a cache path — the
    // pair just can't pre-fill the remote text.
    let is_on_rclone_mount = crate::rclone_ops::is_rclone_fuse_path(local_path);
    let sync_local_path = if is_on_rclone_mount {
        crate::offline::cache_path_for(local_path)
            .to_string_lossy()
            .to_string()
    } else {
        local_path.to_string()
    };

    // Description label
    let intro_text = if is_on_rclone_mount {
        format!(
            "Two-way sync for:\n{local_path}\n\nThis folder lives on an rclone mount, so Wayfinder will sync the OFFLINE COPY of this folder with the remote — not the mounted folder itself (that would be syncing the remote with itself). The cache path is:\n{sync_local_path}"
        )
    } else {
        format!("Two-way sync for:\n{local_path}")
    };
    let intro = gtk::Label::builder()
        .label(intro_text)
        .xalign(0.0)
        .wrap(true)
        .build();
    vbox.append(&intro);

    // Remote selector — combo of detected remotes plus a free-text option
    let remote_label = gtk::Label::builder()
        .label("Rclone remote and path (e.g. Proton:Work)")
        .xalign(0.0)
        .build();
    let remote_entry = gtk::Entry::builder().hexpand(true).build();
    if let Some(ref existing) = existing {
        remote_entry.set_text(&existing.remote);
    } else {
        // Build a sensible default by inspecting the local path:
        //
        // 1. If the local path is INSIDE an rclone FUSE mount, derive
        //    the exact remote subpath from the mount → that path is
        //    almost certainly what the user wants.
        //
        // 2. Otherwise, suggest "FirstRemote:<folder-basename>" so the
        //    user only needs to confirm or edit the path component.
        if let Some(suggestion) = suggest_remote_for_local(local_path) {
            remote_entry.set_text(&suggestion);
        } else {
            let remotes = list_remotes();
            if let Some(first) = remotes.first() {
                let basename = std::path::Path::new(local_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Folder".to_string());
                remote_entry.set_text(&format!("{first}{basename}"));
            }
        }
    }
    remote_entry.update_property(&[gtk::accessible::Property::Label(
        "Rclone remote path, format remote:path",
    )]);
    vbox.append(&remote_label);
    vbox.append(&remote_entry);

    // Show available remotes for reference
    let remotes = list_remotes();
    if !remotes.is_empty() {
        let hint = gtk::Label::builder()
            .label(format!(
                "Available remotes: {}",
                remotes
                    .iter()
                    .map(|r| r.trim_end_matches(':').to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .xalign(0.0)
            .css_classes(["dim-label"])
            .wrap(true)
            .build();
        vbox.append(&hint);
    }

    // Status / last sync time
    let status_label = gtk::Label::builder()
        .label(if let Some(ref p) = existing {
            if p.last_sync_time > 0 {
                let age = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
                    - p.last_sync_time;
                format!(
                    "Last synced {} ago{}",
                    format_duration(age),
                    if p.initial_resync_done {
                        ""
                    } else {
                        " (initial resync pending)"
                    }
                )
            } else {
                "Never synced".to_string()
            }
        } else {
            "Not yet configured".to_string()
        })
        .xalign(0.0)
        .wrap(true)
        .build();
    status_label.update_property(&[gtk::accessible::Property::Label("Sync status")]);
    vbox.append(&status_label);

    // Buttons
    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_box.set_halign(gtk::Align::End);
    button_box.set_margin_top(12);

    let cancel_btn = gtk::Button::with_label("Close");
    let remove_btn = gtk::Button::with_label("Remove pairing");
    let sync_btn = gtk::Button::with_label("Save and sync now");
    sync_btn.add_css_class("suggested-action");

    if editing {
        button_box.append(&remove_btn);
    }
    button_box.append(&cancel_btn);
    button_box.append(&sync_btn);

    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));
    dlg.set_default_widget(Some(&sync_btn));

    // Cancel
    {
        let d = dlg.clone();
        cancel_btn.connect_clicked(move |_| d.close());
    }

    // Remove
    {
        let d = dlg.clone();
        let p = local_path.to_string();
        let parent = parent.clone();
        remove_btn.connect_clicked(move |_| {
            match remove_pair(&p) {
                Ok(()) => parent.announce(
                    "Sync pairing removed. Files are unchanged.",
                    AccessibleAnnouncementPriority::Medium,
                ),
                Err(e) => parent.announce(
                    &format!("Failed to remove pairing: {e}"),
                    AccessibleAnnouncementPriority::High,
                ),
            }
            d.close();
        });
    }

    // Save and sync
    {
        let d = dlg.clone();
        let p = local_path.to_string();
        let sync_local = sync_local_path.clone();
        let parent = parent.clone();
        let entry = remote_entry.clone();
        sync_btn.connect_clicked(move |_| {
            let remote = entry.text().to_string();
            let remote = remote.trim();

            // Validation: must look like remote:path, with a path component
            // after the colon. Pointing bisync at a remote root (e.g. just
            // "Proton:") would walk the entire remote and is almost certainly
            // a mistake — block it explicitly.
            if remote.is_empty() {
                parent.announce(
                    "Remote is empty. Use the form remote:path, e.g. Proton:Work",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }
            let Some((remote_name, remote_path)) = remote.split_once(':') else {
                parent.announce(
                    "Remote must contain a colon. Use the form remote:path, e.g. Proton:Work",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            };
            if remote_name.is_empty() {
                parent.announce(
                    "Remote name is missing before the colon",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }
            let trimmed_path = remote_path.trim_matches('/');
            if trimmed_path.is_empty() {
                parent.announce(
                    "Remote path cannot be empty. Add a subfolder after the colon, e.g. Proton:Documents/German. Syncing against a remote root is too dangerous.",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }

            // Final guard 1: refuse to bisync a FUSE-mounted local path against
            // the same remote it's mounted from. By the time we get here we
            // should already have redirected to the cache path, but check
            // again in case someone constructed a pathological pair manually.
            if let Some(target) = crate::rclone_ops::rclone_for_path(&sync_local) {
                if target.full_remote_spec.eq_ignore_ascii_case(remote) {
                    parent.announce(
                        "Refusing to sync a folder with itself: the local path is already mounted from this same remote. Pick a different remote path or use the offline-cache flow.",
                        AccessibleAnnouncementPriority::High,
                    );
                    return;
                }
            }

            // Final guard 2: if the kernel reports the local sync path as a
            // fuse.rclone mount (the dangerous case) refuse to proceed. The
            // upstream redirect should have rewritten this to a cache path
            // already, so reaching here means the mount was set up after the
            // dialog opened or some path manipulation happened. Either way,
            // running bisync against a FUSE-mounted path that backs onto the
            // same provider risks data loss.
            if crate::rclone_ops::is_rclone_fuse_path(&sync_local) {
                parent.announce(
                    "Refusing to sync: the local path is on an rclone FUSE mount. This would sync the remote with itself, risking data loss. Close this dialog and try again — Wayfinder will redirect to the offline cache path.",
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }

            if let Err(e) = set_pair(&p, &sync_local, remote) {
                parent.announce(
                    &format!("Failed to save pairing: {e}"),
                    AccessibleAnnouncementPriority::High,
                );
                return;
            }
            d.close();
            // Trigger the sync
            trigger_sync(&parent, &p);
        });
    }

    // Escape to cancel
    {
        let d = dlg.clone();
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dlg.add_controller(key_ctrl);
    }

    // Restore focus on close
    dlg.connect_close_request(move |_| {
        on_close();
        glib::Propagation::Proceed
    });

    dlg.present();
    remote_entry.grab_focus();
}

/// Run a sync via the shared `rclone_ops::progress::show` dialog. Builds
/// bisync args from the configured pair, dispatches `rclone bisync`, and
/// reports conflict count in the success announcement.
pub fn trigger_sync(parent_window: &gtk::Window, local_path: &str) {
    let Some(pair) = pair_for(local_path) else {
        parent_window.announce(
            "No sync pair configured for this folder.",
            AccessibleAnnouncementPriority::High,
        );
        return;
    };
    if !rclone_available() {
        parent_window.announce(
            "rclone is not installed.",
            AccessibleAnnouncementPriority::High,
        );
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&pair.sync_local_path) {
        parent_window.announce(
            &format!("Cannot create local sync dir: {e}"),
            AccessibleAnnouncementPriority::High,
        );
        return;
    }

    let args = super::bisync_args(&pair);
    let local_owned = local_path.to_string();

    let labels = crate::rclone_ops::progress::ProgressLabels {
        title: "Syncing",
        target_description: format!("Syncing {local_path}"),
        initial_status: "Starting rclone bisync. Listing remote — this can take a while.",
        cancel_label: "Cancel sync",
        success_message: Box::new(move |r| {
            // Stamp the pair as synced now that rclone exited cleanly.
            super::record_successful_sync(&local_owned);
            let conflicts = super::count_conflicts(&r.stdout, &r.stderr);
            if conflicts > 0 {
                format!(
                    "Sync complete with {conflicts} conflict(s). Files with .conflict-local or .conflict-remote suffixes preserve both versions."
                )
            } else {
                "Sync complete.".to_string()
            }
        }),
        fail_prefix: "Sync failed. Last messages:",
        error_prefix: "Sync error:",
        cancel_announce: "Cancelling sync. Waiting for rclone to exit.",
    };

    crate::rclone_ops::progress::show(
        parent_window,
        labels,
        summarise_line,
        None,
        move |tx, _cancel| {
            if let Err(e) = crate::rclone_ops::run_streaming(&args, tx.clone()) {
                let _ = tx.send(crate::rclone_ops::RcloneEvent::Finished(Err(e)));
            }
        },
    );
}

/// Pull a short, screen-reader-friendly status line out of a raw rclone
/// log line. Returns None for noise we can skip.
fn summarise_line(line: &str) -> Option<String> {
    // rclone --stats-one-line output:
    // "Transferred:   5.123 MiB / 12.456 MiB, 41%, 1.2 MiB/s, ETA 6s"
    if line.starts_with("Transferred:") {
        return Some(line.trim().to_string());
    }
    // Per-file actions: "INFO  : foo.txt: Copied (new)"
    if let Some(idx) = line.find(": Copied") {
        let path = &line[..idx];
        let path = path.rsplit(": ").next().unwrap_or(path);
        return Some(format!("Copied {path}"));
    }
    if let Some(idx) = line.find(": Deleted") {
        let path = &line[..idx];
        let path = path.rsplit(": ").next().unwrap_or(path);
        return Some(format!("Deleted {path}"));
    }
    // Bisync phase markers
    if line.contains("Bisync started") {
        return Some("Bisync started".to_string());
    }
    if line.contains("Listing files") || line.contains("Building file lists") {
        return Some("Listing files on both sides...".to_string());
    }
    if line.contains("Comparing") {
        return Some("Comparing changes...".to_string());
    }
    if line.contains("Resync") {
        return Some("Initial resync — this is the slow first run".to_string());
    }
    if line.contains("Bisync successful") {
        return Some("Bisync successful, finalising".to_string());
    }
    None
}

fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} second(s)")
    } else if secs < 3600 {
        format!("{} minute(s)", secs / 60)
    } else if secs < 86400 {
        format!("{} hour(s)", secs / 3600)
    } else {
        format!("{} day(s)", secs / 86400)
    }
}

/// Suggest a remote path for a given local directory.
///
/// Delegates to `crate::rclone_ops::rclone_for_path` so this and offline
/// pinning agree on what remote backs a given local path.
fn suggest_remote_for_local(local_path: &str) -> Option<String> {
    crate::rclone_ops::rclone_for_path(local_path).map(|t| t.full_remote_spec)
}

