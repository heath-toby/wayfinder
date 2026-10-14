//! Copying and moving, with one progress window for everything pasted (as
//! the Finder does): "Preparing to copy 5,283 items to Files" while it
//! counts what's to be copied, then "Copying 5,280 items to Files",
//! counting down as each is done, with "250 MB of 2.5 GB, about 2 minutes
//! remaining, 20 MB/s" in the field below.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use super::{ConflictChoice, Placement};

/// How far a copy or move has got: shared by its worker, which counts,
/// and its window, which shows it.
#[derive(Default)]
pub struct Tally {
    /// Still counting what's to be copied.
    pub preparing: bool,
    /// Files, folders and links to copy, all told.
    pub items_total: u64,
    pub items_done: u64,
    /// Bytes in the files to copy.
    pub bytes_total: u64,
    /// Bytes in the files copied.
    pub bytes_done: u64,
    /// Bytes copied of the file being copied.
    pub current: u64,
}

pub type SharedTally = Arc<Mutex<Tally>>;

/// One item to copy or move, any clash settled.
struct Planned {
    source: PathBuf,
    dest: PathBuf,
    replace: bool,
}

/// How an item went.
struct Outcome {
    /// Its name where it went (a kept-both copy has a new one).
    name: String,
    result: Result<(), String>,
    cancelled: bool,
}

/// Called with whether everything was copied or moved.
pub type TransferDone = Box<dyn FnOnce(bool) + 'static>;

/// Copy (or move) `sources` into `dest_dir`: each clash asked about in
/// turn, then all in one progress window. `on_done` is told whether every
/// one was copied (none skipped, refused, cancelled or failed).
pub fn transfer(
    sources: Vec<gio::File>,
    dest_dir: &gio::File,
    parent: &gtk::Window,
    is_move: bool,
    on_done: Option<TransferDone>,
) {
    let total = sources.len();
    plan(
        sources.into(),
        Vec::new(),
        dest_dir.clone(),
        parent.clone(),
        is_move,
        Box::new(move |planned, parent, dest_dir| {
            if planned.is_empty() {
                if let Some(cb) = on_done {
                    cb(false);
                }
                return;
            }
            let all_planned = planned.len() == total;
            run(
                planned,
                &dest_dir,
                &parent,
                is_move,
                Box::new(move |all_done| {
                    if let Some(cb) = on_done {
                        cb(all_planned && all_done);
                    }
                }),
            );
        }),
    );
}

type Planning = Box<dyn FnOnce(Vec<Planned>, gtk::Window, gio::File) + 'static>;

/// Settle each of `queue` in turn — refused (into itself, say), skipped,
/// replacing, or kept both under a new name — then `done` with what's to
/// be done. (One question at a time, not one window per item at once.)
fn plan(
    mut queue: VecDeque<gio::File>,
    mut planned: Vec<Planned>,
    dest_dir: gio::File,
    parent: gtk::Window,
    is_move: bool,
    done: Planning,
) {
    let Some(source) = queue.pop_front() else {
        done(planned, parent, dest_dir);
        return;
    };
    let next = move |planned: Vec<Planned>, parent: gtk::Window, dest_dir: gio::File, done| {
        plan(queue, planned, dest_dir, parent, is_move, done)
    };
    let verb = if is_move { "move" } else { "copy" };

    let Some(name) = source.basename() else {
        next(planned, parent, dest_dir, done);
        return;
    };
    let dest = dest_dir.child(&name);
    let display_name = name.to_string_lossy().to_string();
    // Only files with a place on disk (a phone or network location
    // without one can't be copied this way): say so, not just stop.
    let (Some(src_path), Some(dest_path), Some(dest_dir_path)) =
        (source.path(), dest.path(), dest_dir.path())
    else {
        parent.announce(
            &format!("Can't {verb} {display_name}: it isn't on a drive Wayfinder can reach"),
            AccessibleAnnouncementPriority::High,
        );
        next(planned, parent, dest_dir, done);
        return;
    };

    // A folder can't go inside itself, and moving something to where it
    // already is does nothing.
    let refusal = match super::placement(&src_path, &dest_path) {
        Placement::IntoItself => Some(format!("Cannot {verb} {display_name} into itself")),
        Placement::Same if is_move => Some(format!("{display_name} is already here")),
        _ => None,
    };
    if let Some(msg) = refusal {
        parent.announce(&msg, AccessibleAnnouncementPriority::High);
        next(planned, parent, dest_dir, done);
        return;
    }

    // Checking for a clash touches the destination, which may be on a slow
    // mount, so it happens off the main thread.
    let waiting = crate::background::waiting_message(&dest_dir_path.to_string_lossy());
    let parent_slow = parent.clone();
    let dest_for_check = dest.clone();
    crate::background::run(
        move || super::exists_here(&dest_for_check),
        move |exists| {
            if !exists {
                planned.push(Planned {
                    source: src_path,
                    dest: dest_path,
                    replace: false,
                });
                next(planned, parent, dest_dir, done);
                return;
            }
            let parent_for_choice = parent.clone();
            let dialog_name = display_name.clone();
            super::show_conflict_dialog(&dialog_name, &dest, &parent, move |choice| {
                match choice {
                    ConflictChoice::Skip => {
                        parent_for_choice.announce(
                            &format!("Skipped {display_name}"),
                            AccessibleAnnouncementPriority::Medium,
                        );
                    }
                    // Replacing would delete the source itself (or the
                    // folder holding it) before copying it.
                    ConflictChoice::Replace
                        if super::placement(&src_path, &dest_path) != Placement::Elsewhere =>
                    {
                        parent_for_choice.announce(
                            &format!("Cannot replace {display_name} with itself"),
                            AccessibleAnnouncementPriority::High,
                        );
                    }
                    ConflictChoice::Replace => planned.push(Planned {
                        source: src_path,
                        dest: dest_path,
                        replace: true,
                    }),
                    ConflictChoice::Rename(renamed) => {
                        if let Some(renamed) = renamed.path() {
                            planned.push(Planned {
                                source: src_path,
                                dest: renamed,
                                replace: false,
                            });
                        }
                    }
                }
                next(planned, parent_for_choice, dest_dir, done);
            });
        },
        move || {
            if let Some(msg) = waiting {
                parent_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
            }
        },
    );
}

/// Copy or move `planned` into `dest_dir`, in one progress window; `done`
/// is told whether all were.
fn run(
    planned: Vec<Planned>,
    dest_dir: &gio::File,
    parent: &gtk::Window,
    is_move: bool,
    done: Box<dyn FnOnce(bool) + 'static>,
) {
    let dest_label = dest_dir
        .path()
        .map(|p| crate::file_model::display_name(&p.to_string_lossy()))
        .unwrap_or_default();
    let window_title = if is_move { "Moving" } else { "Copying" };

    let dlg = gtk::Window::builder()
        .title(window_title)
        .modal(false)
        .transient_for(parent)
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
    let summary = field(window_title, &summary_phrase(true, is_move, 0, &dest_label));
    let progress_bar = gtk::ProgressBar::new();
    progress_bar.update_property(&[gtk::accessible::Property::Label("Progress")]);
    let status = field("Details", "Preparing…");

    let cancellable = gio::Cancellable::new();
    let cancel_button = gtk::Button::with_label("Cancel");
    let c = cancellable.clone();
    let d = dlg.clone();
    cancel_button.connect_clicked(move |_| {
        c.cancel();
        d.set_title(Some("Cancelling..."));
    });

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_box.set_halign(gtk::Align::End);
    button_box.append(&crate::operations::background_button(&dlg, parent));
    button_box.append(&cancel_button);

    vbox.append(&summary);
    vbox.append(&progress_bar);
    vbox.append(&status);
    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));

    // Closing the window cancels, as Cancel does. (It's put away until the
    // copy has stopped, which then closes it and says so.)
    crate::operations::register(&dlg);
    let c = cancellable.clone();
    dlg.connect_close_request(move |d| {
        if !crate::operations::is_running(d) {
            return glib::Propagation::Proceed;
        }
        c.cancel();
        d.set_visible(false);
        glib::Propagation::Stop
    });

    // Shown only if it takes a moment: a small copy is just said when done.
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
    let (tx, rx) = std::sync::mpsc::channel::<Vec<Outcome>>();
    {
        let tally = tally.clone();
        let cancellable = cancellable.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(planned, is_move, &tally, &cancellable));
        });
    }

    // What the window shows, every quarter second; the speed from the last
    // few seconds' progress.
    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    let parent = parent.clone();
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(250), move || {
        if let Ok(outcomes) = rx.try_recv() {
            crate::operations::close(&dlg);
            let (msg, trouble) = outcome_phrase(&outcomes, is_move, &dest_label);
            let priority = if trouble {
                AccessibleAnnouncementPriority::High
            } else {
                AccessibleAnnouncementPriority::Medium
            };
            parent.announce(&msg, priority);
            let all_done = outcomes.iter().all(|o| o.result.is_ok());
            if let Some(done) = done.take() {
                done(all_done);
            }
            return glib::ControlFlow::Break;
        }

        let t = tally.lock().unwrap_or_else(|e| e.into_inner());
        let remaining = t.items_total.saturating_sub(t.items_done);
        let shown = if t.preparing {
            t.items_total
        } else {
            remaining
        };
        let text = summary_phrase(t.preparing, is_move, shown, &dest_label);
        // (Only when it changes: Orca may read a focused field's changes.)
        if summary.text() != text {
            summary.set_text(&text);
        }
        if t.preparing {
            progress_bar.pulse();
        } else {
            let copied = (t.bytes_done + t.current).min(t.bytes_total);
            let now = Instant::now();
            samples.push_back((now, copied));
            while samples
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(5))
            {
                samples.pop_front();
            }
            let speed = samples.front().and_then(|(at, bytes)| {
                let secs = now.duration_since(*at).as_secs_f64();
                (secs >= 1.0).then(|| copied.saturating_sub(*bytes) as f64 / secs)
            });
            if t.bytes_total > 0 {
                progress_bar.set_fraction(copied as f64 / t.bytes_total as f64);
            } else if t.items_total > 0 {
                progress_bar.set_fraction(t.items_done as f64 / t.items_total as f64);
            }
            let text = status_phrase(copied, t.bytes_total, speed);
            if status.text() != text {
                status.set_text(&text);
            }
        }
        glib::ControlFlow::Continue
    });
}

/// The worker: moves that are only renames first (instant), then what's
/// left counted, then copied — each in turn, going on past one that fails,
/// stopping if cancelled.
fn work(
    planned: Vec<Planned>,
    is_move: bool,
    tally: &SharedTally,
    cancellable: &gio::Cancellable,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let mut to_copy = Vec::new();
    let outcome = |item: &Planned, result: Result<(), glib::Error>| Outcome {
        name: item
            .dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        cancelled: result
            .as_ref()
            .is_err_and(|e| e.matches(gio::IOErrorEnum::Cancelled)),
        result: result.map_err(|e| e.to_string()),
    };
    let cancelled = || glib::Error::new(gio::IOErrorEnum::Cancelled, "Operation cancelled");

    for item in planned {
        if cancellable.is_cancelled() {
            outcomes.push(outcome(&item, Err(cancelled())));
            continue;
        }
        match prepare(&item, is_move, cancellable) {
            Ok(true) => outcomes.push(outcome(&item, Ok(()))),
            Ok(false) => to_copy.push(item),
            Err(e) => outcomes.push(outcome(&item, Err(e))),
        }
    }

    for item in &to_copy {
        count(&gio::File::for_path(&item.source), tally, cancellable);
    }
    tally.lock().unwrap_or_else(|e| e.into_inner()).preparing = false;

    for item in to_copy {
        let result = if cancellable.is_cancelled() {
            Err(cancelled())
        } else {
            copy_item(&item, is_move, tally, cancellable)
        };
        outcomes.push(outcome(&item, result));
    }
    outcomes
}

/// Get `item` ready: refuse it if (by real paths) it would go into itself
/// or replace itself; clear what it replaces, if that must go first; and
/// for a move, move it now if that's a rename on one drive. `Ok(true)` if
/// that did it, `Ok(false)` if it's to be copied.
fn prepare(
    item: &Planned,
    is_move: bool,
    cancellable: &gio::Cancellable,
) -> Result<bool, glib::Error> {
    let (src, dst) = (
        gio::File::for_path(&item.source),
        gio::File::for_path(&item.dest),
    );
    // Checked before on the paths as shown; again here on real paths, as
    // a folder reached through a link is the same folder.
    if let Some(msg) = super::real_placement_refusal(
        &item.source.to_string_lossy(),
        &item.dest.to_string_lossy(),
        is_move,
        item.replace,
    ) {
        return Err(glib::Error::new(gio::IOErrorEnum::InvalidArgument, &msg));
    }
    // Replacing something of the other kind (a file with a folder, or a
    // folder with a file): GIO won't put one over the other, so the old
    // one goes first. (A folder over a folder is replaced whole by the copy
    // itself.)
    if item.replace {
        let kind = |f: &gio::File| {
            f.query_file_type(
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )
        };
        let (src_kind, dst_kind) = (kind(&src), kind(&dst));
        if super::clear_before_replace(src_kind, dst_kind, is_move) {
            if dst_kind == gio::FileType::Directory {
                super::delete_recursive_gio(&dst, cancellable)?;
            } else {
                dst.delete(Some(cancellable))?;
            }
        }
    }
    if !is_move {
        return Ok(false);
    }
    let flags = if item.replace {
        gio::FileCopyFlags::OVERWRITE
    } else {
        gio::FileCopyFlags::NONE
    };
    match src.move_(
        &dst,
        flags | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE,
        Some(cancellable),
        Some(&mut |_, _| {}),
    ) {
        Ok(()) => Ok(true),
        // To another drive: copied, then deleted.
        Err(e)
            if e.matches(gio::IOErrorEnum::NotSupported)
                || e.matches(gio::IOErrorEnum::WouldMerge)
                || e.matches(gio::IOErrorEnum::WouldRecurse) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Count `file` and everything in it into the tally (items, and the bytes
/// in its files). Not following links; what can't be looked into counts
/// as itself only.
fn count(file: &gio::File, tally: &SharedTally, cancellable: &gio::Cancellable) {
    let Ok(info) = file.query_info(
        "standard::type,standard::size",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    ) else {
        tally.lock().unwrap_or_else(|e| e.into_inner()).items_total += 1;
        return;
    };
    count_in(file, info.file_type(), info.size(), tally, cancellable);
}

fn count_in(
    file: &gio::File,
    kind: gio::FileType,
    size: i64,
    tally: &SharedTally,
    cancellable: &gio::Cancellable,
) {
    {
        let mut t = tally.lock().unwrap_or_else(|e| e.into_inner());
        t.items_total += 1;
        if kind == gio::FileType::Regular {
            t.bytes_total += size.max(0) as u64;
        }
    }
    if kind != gio::FileType::Directory {
        return;
    }
    let Ok(children) = file.enumerate_children(
        "standard::name,standard::type,standard::size",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    ) else {
        return;
    };
    while let Ok(Some(info)) = children.next_file(Some(cancellable)) {
        count_in(
            &file.child(info.name()),
            info.file_type(),
            info.size(),
            tally,
            cancellable,
        );
    }
}

/// Copy `item` (a move: copy, then delete the original — to another drive,
/// where it can't just be renamed).
fn copy_item(
    item: &Planned,
    is_move: bool,
    tally: &SharedTally,
    cancellable: &gio::Cancellable,
) -> Result<(), glib::Error> {
    let (src, dst) = (
        gio::File::for_path(&item.source),
        gio::File::for_path(&item.dest),
    );
    let flags = if item.replace {
        gio::FileCopyFlags::OVERWRITE
    } else {
        gio::FileCopyFlags::NONE
    };
    let is_dir = src.query_file_type(
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    ) == gio::FileType::Directory;
    match (is_move, is_dir) {
        // Before copying: a drive mounted inside would be copied whole,
        // only for deleting it to be refused.
        (true, true) => super::refuse_mounts_inside(&src)
            .and_then(|()| {
                super::copy_directory_recursive(&src, &dst, cancellable, tally, item.replace, true)
            })
            .and_then(|()| super::delete_recursive_gio(&src, cancellable)),
        (true, false) => super::copy_keeping_links(
            &src,
            &dst,
            flags | gio::FileCopyFlags::ALL_METADATA,
            cancellable,
            tally,
        )
        .and_then(|()| src.delete(Some(cancellable))),
        (false, true) => {
            super::copy_directory_recursive(&src, &dst, cancellable, tally, item.replace, false)
        }
        (false, false) => super::copy_keeping_links(&src, &dst, flags, cancellable, tally),
    }
}

/// What the window's first field says: "Preparing to copy 5,283 items to
/// Files" while counting, then "Copying 5,280 items to Files", `items`
/// being those left.
pub(crate) fn summary_phrase(preparing: bool, is_move: bool, items: u64, dest: &str) -> String {
    let (verb, doing) = if is_move {
        ("move", "Moving")
    } else {
        ("copy", "Copying")
    };
    match (preparing, items) {
        (true, 0) => format!("Preparing to {verb} to {dest}"),
        (true, n) => format!("Preparing to {verb} {} to {dest}", items_phrase(n)),
        (false, n) => format!("{doing} {} to {dest}", items_phrase(n)),
    }
}

/// "1 item", "5,283 items".
pub(super) fn items_phrase(n: u64) -> String {
    if n == 1 {
        "1 item".to_string()
    } else {
        format!("{} items", with_commas(n))
    }
}

pub(super) fn with_commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What the details field says: "250 MB of 2.5 GB, about 2 minutes
/// remaining, 20 MB/s" (`speed` in bytes a second, once known).
pub(crate) fn status_phrase(done: u64, total: u64, speed: Option<f64>) -> String {
    let sizes = format!("{} of {}", size_phrase(done), size_phrase(total));
    match speed.filter(|s| *s >= 1.0) {
        Some(speed) => {
            let left = total.saturating_sub(done) as f64 / speed;
            format!(
                "{sizes}, {}, {}/s",
                time_phrase(left.round() as u64),
                size_phrase(speed as u64)
            )
        }
        None => format!("{sizes}, estimating time remaining"),
    }
}

/// A size: whole numbers from 100 up, one decimal place below ("250 MB",
/// "2.5 GB").
pub(crate) fn size_phrase(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        let text = format!("{value:.1}");
        let text = text.strip_suffix(".0").unwrap_or(&text);
        format!("{text} {}", UNITS[unit])
    }
}

/// "about 5 seconds remaining", "about 2 minutes remaining", "about 1 hour
/// and 10 minutes remaining".
pub(super) fn time_phrase(secs: u64) -> String {
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    if secs < 60 {
        // To the nearest 5 seconds: it changes as often as it's read.
        let secs = (secs.div_ceil(5) * 5).max(5);
        if secs == 60 {
            return "about 1 minute remaining".to_string();
        }
        format!("about {} remaining", plural(secs, "second"))
    } else if secs < 3600 {
        let mins = (secs + 30) / 60;
        if mins == 60 {
            return "about 1 hour remaining".to_string();
        }
        format!("about {} remaining", plural(mins, "minute"))
    } else {
        let mins = (secs + 30) / 60;
        let (hours, mins) = (mins / 60, mins % 60);
        if mins == 0 {
            format!("about {} remaining", plural(hours, "hour"))
        } else {
            format!(
                "about {} and {} remaining",
                plural(hours, "hour"),
                plural(mins, "minute")
            )
        }
    }
}

/// What's said when it's over, and whether something went wrong.
fn outcome_phrase(outcomes: &[Outcome], is_move: bool, dest: &str) -> (String, bool) {
    let (done_verb, verb) = if is_move {
        ("Moved", "move")
    } else {
        ("Copied", "copy")
    };
    let total = outcomes.len();
    let ok = outcomes.iter().filter(|o| o.result.is_ok()).count();
    let of_total = |ok: usize| format!("{ok} of {total} {}", done_verb.to_lowercase());
    if ok == total {
        return match outcomes {
            [one] => (format!("{done_verb} {}", one.name), false),
            _ => (format!("{done_verb} {total} items to {dest}"), false),
        };
    }
    if outcomes.iter().any(|o| o.cancelled) {
        let msg = if ok > 0 {
            format!("Cancelled. {}", of_total(ok))
        } else {
            "Cancelled".to_string()
        };
        return (msg, false);
    }
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| o.result.is_err()).collect();
    let first = failed[0];
    let mut msg = format!(
        "Couldn't {verb} {}: {}",
        first.name,
        first.result.as_ref().err().cloned().unwrap_or_default()
    );
    if failed.len() > 1 {
        msg.push_str(&format!(", nor {} more", failed.len() - 1));
    }
    if ok > 0 {
        msg.push_str(&format!(". {}", of_total(ok)));
    }
    (msg, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_says_what_and_how_far_as_the_finder_does() {
        assert_eq!(
            summary_phrase(true, false, 5283, "Files"),
            "Preparing to copy 5,283 items to Files"
        );
        assert_eq!(
            summary_phrase(true, true, 0, "Files"),
            "Preparing to move to Files"
        );
        assert_eq!(
            summary_phrase(false, false, 1, "Files"),
            "Copying 1 item to Files"
        );
        assert_eq!(items_phrase(1_234_567), "1,234,567 items");
        assert_eq!(items_phrase(999), "999 items");
        let mb = 1024 * 1024;
        assert_eq!(
            status_phrase(250 * mb, 2560 * mb, Some(20.0 * mb as f64)),
            "250 MB of 2.5 GB, about 2 minutes remaining, 20 MB/s"
        );
        assert_eq!(
            status_phrase(0, 2 * mb, None),
            "0 bytes of 2 MB, estimating time remaining"
        );
        assert_eq!(time_phrase(3), "about 5 seconds remaining");
        assert_eq!(time_phrase(42), "about 45 seconds remaining");
        assert_eq!(time_phrase(58), "about 1 minute remaining");
        assert_eq!(time_phrase(4200), "about 1 hour and 10 minutes remaining");
        assert_eq!(time_phrase(7200), "about 2 hours remaining");
    }

    #[test]
    fn the_end_is_said_in_a_sentence() {
        let ok = |name: &str| Outcome {
            name: name.to_string(),
            result: Ok(()),
            cancelled: false,
        };
        let failed = |name: &str| Outcome {
            name: name.to_string(),
            result: Err("No space left on device".to_string()),
            cancelled: false,
        };
        assert_eq!(
            outcome_phrase(&[ok("a.txt")], false, "Files").0,
            "Copied a.txt"
        );
        assert_eq!(
            outcome_phrase(&[ok("a"), ok("b")], true, "Files").0,
            "Moved 2 items to Files"
        );
        assert_eq!(
            outcome_phrase(&[ok("a"), failed("b"), failed("c")], false, "Files"),
            (
                "Couldn't copy b: No space left on device, nor 1 more. 1 of 3 copied".to_string(),
                true
            )
        );
    }

    /// A folder's items, files and bytes are all counted.
    #[test]
    fn what_is_to_be_copied_is_counted() {
        let d = std::env::temp_dir().join(format!("wayfinder-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a"), [0u8; 10]).unwrap();
        std::fs::write(d.join("sub/b"), [0u8; 5]).unwrap();
        let tally = SharedTally::default();
        count(&gio::File::for_path(&d), &tally, &gio::Cancellable::new());
        let _ = std::fs::remove_dir_all(&d);
        let t = tally.lock().unwrap();
        assert_eq!((t.items_total, t.bytes_total), (4, 15));
    }
}
