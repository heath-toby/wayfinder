//! Deleting permanently, with a progress window (as the Finder does):
//! "Preparing to delete 5,283 items" while it counts them, then "Deleting
//! 5,280 items", counting down as each file and folder goes, with "1,200
//! of 5,283 items deleted, about 30 seconds remaining" in the field below.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use super::transfer::{items_phrase, time_phrase, with_commas, SharedTally};

/// Called when `delete_with_progress` finishes: how many of the paths were
/// deleted, how many couldn't be, and why the first couldn't. (Fewer than
/// all, together: cancelled.)
pub type DeleteCompleteCallback = Box<dyn FnOnce(usize, usize, Option<String>) + 'static>;

/// Why deleting stopped.
enum Stop {
    Cancelled,
    Failed(String),
}

/// Permanently delete `paths` in a background thread, with a progress
/// window (shown if it takes a moment) and Cancel. `on_complete` runs on
/// the main thread when it's over, however it went.
pub fn delete_with_progress(
    paths: Vec<String>,
    parent_window: &gtk::Window,
    on_complete: Option<DeleteCompleteCallback>,
) {
    if paths.is_empty() {
        if let Some(cb) = on_complete {
            cb(0, 0, None);
        }
        return;
    }

    let dlg = gtk::Window::builder()
        .title("Deleting")
        .modal(false)
        .transient_for(parent_window)
        .default_width(450)
        .resizable(false)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    // Read-only fields, so Orca users can Tab to them and read them as
    // they change.
    let field = |label: &str, text: &str| {
        let entry = gtk::Entry::builder()
            .editable(false)
            .can_focus(true)
            .text(text)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label(label)]);
        entry
    };
    let summary = field("Deleting", &summary_phrase(true, 0));
    let progress_bar = gtk::ProgressBar::new();
    progress_bar.update_property(&[gtk::accessible::Property::Label("Progress")]);
    let status = field("Details", "Preparing…");

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_button = gtk::Button::with_label("Cancel");
    cancel_button.update_property(&[gtk::accessible::Property::Label("Cancel deletion")]);
    let c = cancelled.clone();
    let d = dlg.clone();
    cancel_button.connect_clicked(move |_| {
        c.store(true, Ordering::Relaxed);
        d.set_title(Some("Cancelling..."));
    });

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_box.set_halign(gtk::Align::End);
    button_box.append(&crate::operations::background_button(&dlg, parent_window));
    button_box.append(&cancel_button);

    vbox.append(&summary);
    vbox.append(&progress_bar);
    vbox.append(&status);
    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));

    // Closing the window cancels, as Cancel does: it stays until the file
    // being deleted is gone. Closed again meanwhile (one stuck on a frozen
    // mount), it's put away, not kept up for good.
    crate::operations::register(&dlg);
    let c = cancelled.clone();
    let parent_for_close = parent_window.clone();
    dlg.connect_close_request(move |d| {
        if !crate::operations::is_running(d) {
            return glib::Propagation::Proceed;
        }
        if c.swap(true, Ordering::Relaxed) {
            crate::operations::hide(d, &parent_for_close);
        } else {
            d.set_title(Some("Cancelling..."));
            d.announce("Cancelling", AccessibleAnnouncementPriority::Medium);
        }
        glib::Propagation::Stop
    });

    // Shown only if it takes a moment: a quick delete is just said when done.
    {
        let dlg = dlg.clone();
        let summary = summary.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if crate::operations::is_running(&dlg) && !dlg.is_visible() {
                dlg.present();
                summary.grab_focus();
            }
        });
    }

    let tally = SharedTally::default();
    tally.lock().unwrap_or_else(|e| e.into_inner()).preparing = true;
    let (tx, rx) = std::sync::mpsc::channel::<(usize, usize, Option<String>)>();
    {
        let tally = tally.clone();
        let cancelled = cancelled.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(&paths, &tally, &cancelled));
        });
    }

    // What the window shows, every quarter second; the time left from the
    // last few seconds' pace.
    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    let mut on_complete = on_complete;
    glib::timeout_add_local(Duration::from_millis(250), move || {
        if let Ok((success, failed, why)) = rx.try_recv() {
            crate::operations::close(&dlg);
            if let Some(cb) = on_complete.take() {
                cb(success, failed, why);
            }
            return glib::ControlFlow::Break;
        }
        let t = tally.lock().unwrap_or_else(|e| e.into_inner());
        let shown = if t.preparing {
            t.items_total
        } else {
            t.items_total.saturating_sub(t.items_done)
        };
        let text = summary_phrase(t.preparing, shown);
        // (Only when it changes: Orca may read a focused field's changes.)
        if summary.text() != text {
            summary.set_text(&text);
        }
        if t.preparing {
            progress_bar.pulse();
            return glib::ControlFlow::Continue;
        }
        let now = Instant::now();
        samples.push_back((now, t.items_done));
        while samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(5))
        {
            samples.pop_front();
        }
        let rate = samples.front().and_then(|(at, done)| {
            let secs = now.duration_since(*at).as_secs_f64();
            (secs >= 1.0).then(|| t.items_done.saturating_sub(*done) as f64 / secs)
        });
        if t.items_total > 0 {
            progress_bar.set_fraction(t.items_done as f64 / t.items_total as f64);
        }
        let text = status_phrase(t.items_done, t.items_total, rate);
        if status.text() != text {
            status.set_text(&text);
        }
        glib::ControlFlow::Continue
    });
}

/// The worker: everything counted first, then deleted, one path after
/// another — going on past one that can't be, stopping if cancelled. How
/// many were deleted, how many couldn't be, and why the first couldn't.
fn work(
    paths: &[String],
    tally: &SharedTally,
    cancelled: &AtomicBool,
) -> (usize, usize, Option<String>) {
    // Never into a drive mounted inside (a whole remote, perhaps), judged
    // on the real path: such a path is refused, not counted.
    let refusals: Vec<Option<String>> = paths
        .iter()
        .map(|p| {
            super::refuse_mounts_inside(&gtk::gio::File::for_path(p))
                .err()
                .map(|e| e.to_string())
        })
        .collect();
    for (path, refused) in paths.iter().zip(&refusals) {
        if refused.is_none() && !cancelled.load(Ordering::Relaxed) {
            count(Path::new(path), tally, cancelled);
        }
    }
    tally.lock().unwrap_or_else(|e| e.into_inner()).preparing = false;

    let (mut success, mut failed, mut why) = (0, 0, None);
    let name = |p: &str| {
        Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string())
    };
    for (path, refused) in paths.iter().zip(refusals) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let result = match refused {
            Some(e) => Err(Stop::Failed(e)),
            None => delete(Path::new(path), tally, cancelled),
        };
        match result {
            Ok(()) => success += 1,
            Err(Stop::Cancelled) => break,
            Err(Stop::Failed(e)) => {
                failed += 1;
                why.get_or_insert_with(|| format!("{}: {e}", name(path)));
            }
        }
    }
    (success, failed, why)
}

/// Count `path` and everything in it (not following links).
fn count(path: &Path, tally: &SharedTally, cancelled: &AtomicBool) {
    tally.lock().unwrap_or_else(|e| e.into_inner()).items_total += 1;
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        count(&entry.path(), tally, cancelled);
    }
}

/// Delete `path` — a folder, everything in it first — counting each as it
/// goes (not following links; std::fs works through any FUSE mount with
/// the usual file calls). Stops at the first that can't be deleted, or
/// when cancelled.
fn delete(path: &Path, tally: &SharedTally, cancelled: &AtomicBool) -> Result<(), Stop> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(Stop::Cancelled);
    }
    let failed = |e: std::io::Error| Stop::Failed(e.to_string());
    let meta = std::fs::symlink_metadata(path).map_err(failed)?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).map_err(failed)? {
            delete(&entry.map_err(failed)?.path(), tally, cancelled)?;
        }
        std::fs::remove_dir(path).map_err(failed)?;
    } else {
        std::fs::remove_file(path).map_err(failed)?;
    }
    tally.lock().unwrap_or_else(|e| e.into_inner()).items_done += 1;
    Ok(())
}

/// What the window's first field says: "Preparing to delete 5,283 items"
/// while counting, then "Deleting 5,280 items", `items` being those left.
pub(crate) fn summary_phrase(preparing: bool, items: u64) -> String {
    match (preparing, items) {
        (true, 0) => "Preparing to delete".to_string(),
        (true, n) => format!("Preparing to delete {}", items_phrase(n)),
        (false, n) => format!("Deleting {}", items_phrase(n)),
    }
}

/// What the details field says: "1,200 of 5,283 items deleted, about 30
/// seconds remaining" (`rate` in items a second, once known).
pub(crate) fn status_phrase(done: u64, total: u64, rate: Option<f64>) -> String {
    let deleted = format!("{} of {} deleted", with_commas(done), items_phrase(total));
    match rate.filter(|r| *r > 0.0) {
        Some(rate) => {
            let left = total.saturating_sub(done) as f64 / rate;
            format!("{deleted}, {}", time_phrase(left.round() as u64))
        }
        None => deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_counts_down_what_is_left() {
        assert_eq!(
            summary_phrase(true, 5283),
            "Preparing to delete 5,283 items"
        );
        assert_eq!(summary_phrase(false, 1), "Deleting 1 item");
        assert_eq!(
            status_phrase(1200, 5283, Some(136.0)),
            "1,200 of 5,283 items deleted, about 30 seconds remaining"
        );
        assert_eq!(status_phrase(0, 1, None), "0 of 1 item deleted");
    }

    /// Everything in a folder is counted, and deleted, each one ticking
    /// the count.
    #[test]
    fn a_folder_is_counted_and_deleted_item_by_item() {
        let d = std::env::temp_dir().join(format!("wayfinder-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a"), "").unwrap();
        std::fs::write(d.join("sub/b"), "").unwrap();
        std::os::unix::fs::symlink("/nonexistent", d.join("link")).unwrap();
        let tally = SharedTally::default();
        let stop = AtomicBool::new(false);
        let (ok, failed, _) = work(&[d.to_string_lossy().to_string()], &tally, &stop);
        let gone = !d.exists();
        let t = tally.lock().unwrap();
        assert_eq!((ok, failed, gone), (1, 0, true));
        assert_eq!((t.items_total, t.items_done), (5, 5));
    }
}
