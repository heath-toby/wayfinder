use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

// Simple (non-progress) file operations

/// Result of attempting to trash a file.
pub enum TrashResult {
    /// File was successfully moved to the Bin.
    Trashed,
    /// Trash is not supported (e.g. FUSE mount) — caller should offer
    /// permanent deletion instead.
    NeedsPermanentDelete,
}

/// Try to trash a file; if the filesystem doesn't support trash (e.g. FUSE
/// mounts like rclone), return `NeedsPermanentDelete` so the caller can
/// offer a confirmation dialog.
pub fn trash_or_delete(file: &gio::File) -> Result<TrashResult, glib::Error> {
    match file.trash(gio::Cancellable::NONE) {
        Ok(()) => Ok(TrashResult::Trashed),
        Err(e) if e.matches(gio::IOErrorEnum::NotSupported) => {
            Ok(TrashResult::NeedsPermanentDelete)
        }
        Err(e) => Err(e),
    }
}

/// Delete `file`, and if it's a folder, everything in it — but never go
/// into a drive mounted inside it (a whole remote, perhaps). Checked on
/// the real path, so a folder reached through a link counts too.
pub fn delete_file_recursive(file: &gio::File) -> Result<(), glib::Error> {
    refuse_mounts_inside(file)?;
    delete_tree(file)
}

/// `path` with every link resolved but its last part: a link is judged as
/// itself (deleting or copying it touches only the link), not as the
/// folder it leads to.
fn real_location(path: &std::path::Path) -> Option<std::path::PathBuf> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Some(std::fs::canonicalize(parent).ok()?.join(name)),
        _ => std::fs::canonicalize(path).ok(),
    }
}

/// An error if a filesystem is mounted inside `file` (a folder).
fn refuse_mounts_inside(file: &gio::File) -> Result<(), glib::Error> {
    let Some(path) = file.path() else {
        return Ok(());
    };
    let real = real_location(&path).unwrap_or(path);
    match crate::rclone_ops::mountpoints_inside(&real.to_string_lossy()).first() {
        Some(mount) => Err(glib::Error::new(
            gio::IOErrorEnum::Busy,
            &format!("{mount} is mounted inside it; unmount it first"),
        )),
        None => Ok(()),
    }
}

fn delete_tree(file: &gio::File) -> Result<(), glib::Error> {
    let file_type = file.query_file_type(
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    );
    if file_type == gio::FileType::Directory {
        let enumerator = file.enumerate_children(
            "standard::name,standard::type",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            let child = file.child(info.name());
            delete_tree(&child)?;
        }
    }
    file.delete(gio::Cancellable::NONE)?;
    Ok(())
}

/// Restore a file from the Bin to its original location
pub fn restore_from_trash(file: &gio::File) -> Result<String, glib::Error> {
    let info = file.query_info(
        "trash::orig-path",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;

    let orig_path = info
        .attribute_byte_string("trash::orig-path")
        .map(|p| p.to_string())
        .unwrap_or_default();

    if orig_path.is_empty() {
        return Err(glib::Error::new(
            gio::IOErrorEnum::NotFound,
            "Original path not found in Bin metadata",
        ));
    }

    let dest = gio::File::for_path(&orig_path);

    // Make sure parent directory exists
    if let Some(parent) = dest.parent() {
        let _ = parent.make_directory_with_parents(gio::Cancellable::NONE);
    }

    let dest = get_unique_dest(&dest);
    file.move_(
        &dest,
        gio::FileCopyFlags::NONE,
        gio::Cancellable::NONE,
        None,
    )?;

    Ok(dest
        .path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(orig_path))
}

/// Empty the Bin — permanently delete all items
pub fn empty_trash() -> Result<u32, glib::Error> {
    let trash = gio::File::for_uri("trash:///");
    let enumerator = trash.enumerate_children(
        "standard::name",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;

    let mut count = 0u32;
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
        let child = trash.child(info.name());
        if let Err(e) = delete_file_recursive(&child) {
            log::warn!("Failed to delete Bin item: {e}");
        } else {
            count += 1;
        }
    }

    Ok(count)
}

pub fn rename_file(file: &gio::File, new_name: &str) -> Result<gio::File, glib::Error> {
    let new_file = file.set_display_name(new_name, gio::Cancellable::NONE)?;
    Ok(new_file)
}

/// Callback fired when `delete_with_progress` finishes: receives
/// `(success_count, failed_count)`.
pub type DeleteCompleteCallback = Box<dyn FnOnce(usize, usize) + 'static>;

/// Permanently delete a list of paths in a background thread, with a
/// non-modal progress dialog and Cancel button. Best used for slow
/// filesystems (FUSE, network mounts) where each delete may take seconds.
///
/// `on_complete` runs on the main thread when all deletions finish or the
/// user cancels.
pub fn delete_with_progress(
    paths: Vec<String>,
    parent_window: &gtk::Window,
    on_complete: Option<DeleteCompleteCallback>,
) {
    let total = paths.len();
    if total == 0 {
        if let Some(cb) = on_complete {
            cb(0, 0);
        }
        return;
    }

    // Build the dialog
    let dlg = gtk::Window::builder()
        .title(if total == 1 {
            "Deleting".to_string()
        } else {
            format!("Deleting {total} items")
        })
        .modal(false)
        .transient_for(parent_window)
        .default_width(450)
        .resizable(false)
        .build();
    dlg.update_property(&[gtk::accessible::Property::Label("Delete progress")]);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let title_label = gtk::Label::builder()
        .label(if total == 1 {
            format!("Deleting {}", paths[0])
        } else {
            format!("Deleting {total} items")
        })
        .xalign(0.0)
        .wrap(true)
        .build();
    title_label.update_property(&[gtk::accessible::Property::Label(
        "Delete operation in progress",
    )]);

    let progress_bar = gtk::ProgressBar::new();
    progress_bar.set_show_text(true);
    progress_bar.update_property(&[gtk::accessible::Property::Label("Progress")]);

    let status_entry = gtk::Entry::builder()
        .editable(false)
        .can_focus(true)
        .text("Preparing...")
        .build();
    status_entry.update_property(&[
        gtk::accessible::Property::Label("Delete status"),
        gtk::accessible::Property::Description("Current file being deleted"),
    ]);

    let cancel_button = gtk::Button::with_label("Cancel");
    cancel_button.update_property(&[gtk::accessible::Property::Label("Cancel deletion")]);

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    button_box.set_halign(gtk::Align::End);
    button_box.append(&cancel_button);

    vbox.append(&title_label);
    vbox.append(&progress_bar);
    vbox.append(&status_entry);
    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));

    // Cancellation flag — checked between each file delete in the worker thread
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_btn = cancelled.clone();
    let dlg_for_cancel = dlg.clone();
    cancel_button.connect_clicked(move |_| {
        cancelled_for_btn.store(true, Ordering::Relaxed);
        // Don't close yet — let the worker thread send Done so the dialog
        // closes with consistent state.
        dlg_for_cancel.set_title(Some("Cancelling..."));
    });

    dlg.present();
    status_entry.grab_focus();

    // Channel for streaming progress events
    enum DeleteEvent {
        Progress {
            done: usize,
            total: usize,
            current_path: String,
            ok: bool,
            error: Option<String>,
        },
        Done {
            success: usize,
            failed: usize,
        },
    }

    let (tx, rx) = std::sync::mpsc::channel::<DeleteEvent>();
    let cancelled_for_thread = cancelled.clone();

    // Worker thread: deletes files using std::fs (no GIO needed; works
    // through any FUSE mount that exposes a normal POSIX interface).
    std::thread::spawn(move || {
        let mut success = 0;
        let mut failed = 0;
        for (i, path) in paths.iter().enumerate() {
            if cancelled_for_thread.load(Ordering::Relaxed) {
                break;
            }
            let result = delete_path_recursive_std(std::path::Path::new(path));
            let ok = result.is_ok();
            if ok {
                success += 1;
            } else {
                failed += 1;
            }
            let _ = tx.send(DeleteEvent::Progress {
                done: i + 1,
                total,
                current_path: path.clone(),
                ok,
                error: result.err(),
            });
        }
        let _ = tx.send(DeleteEvent::Done { success, failed });
    });

    // Main thread polls the channel and updates the UI
    let on_complete_cell: std::rc::Rc<std::cell::RefCell<Option<DeleteCompleteCallback>>> =
        std::rc::Rc::new(std::cell::RefCell::new(on_complete));
    let progress_clone = progress_bar.clone();
    let status_clone = status_entry.clone();
    let dlg_clone = dlg.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
        loop {
            match rx.try_recv() {
                Ok(DeleteEvent::Progress {
                    done,
                    total,
                    current_path,
                    ok,
                    error,
                }) => {
                    let fraction = done as f64 / total as f64;
                    progress_clone.set_fraction(fraction);
                    progress_clone.set_text(Some(&format!(
                        "{done} of {total}, {:.0}%",
                        fraction * 100.0
                    )));
                    let basename = std::path::Path::new(&current_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or(current_path);
                    let msg = if ok {
                        format!("Deleted {basename}")
                    } else {
                        format!("Failed: {basename}: {}", error.unwrap_or_default())
                    };
                    status_clone.set_text(&msg);
                }
                Ok(DeleteEvent::Done { success, failed }) => {
                    dlg_clone.close();
                    if let Some(cb) = on_complete_cell.borrow_mut().take() {
                        cb(success, failed);
                    }
                    return glib::ControlFlow::Break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(_) => {
                    dlg_clone.close();
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

/// Recursive delete using std::fs. Works through any FUSE mount that
/// exposes POSIX semantics. Faster startup than GIO for our use case
/// because we skip the full file-info enumeration on each child.
fn delete_path_recursive_std(path: &std::path::Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn create_folder(parent: &gio::File, name: &str) -> Result<gio::File, glib::Error> {
    let folder = parent.child(name);
    folder.make_directory(gio::Cancellable::NONE)?;
    Ok(folder)
}

/// User's choice when a destination file already exists.
enum ConflictChoice {
    Replace,
    Skip,
    Rename(gio::File),
}

/// Show a conflict resolution dialog when the destination already exists.
/// Returns the user's choice via a channel.
fn show_conflict_dialog(
    name: &str,
    dest: &gio::File,
    parent_window: &gtk::Window,
    callback: impl FnOnce(ConflictChoice) + 'static,
) {
    let dialog = gtk::AlertDialog::builder()
        .message(format!("{name} already exists"))
        .detail("What would you like to do?")
        .buttons(["Skip", "Rename (keep both)", "Replace"])
        .cancel_button(0)
        .default_button(0)
        .build();

    let dest = dest.clone();
    dialog.choose(Some(parent_window), gio::Cancellable::NONE, move |result| {
        match result {
            Ok(2) => callback(ConflictChoice::Replace),
            Ok(1) => {
                // Finding a free name probes the destination, which may be
                // on a slow mount.
                crate::background::run(
                    move || get_unique_dest(&dest),
                    move |renamed| callback(ConflictChoice::Rename(renamed)),
                    || {},
                );
            }
            _ => callback(ConflictChoice::Skip),
        }
    });
}

/// Copy with a progress dialog.
/// `on_complete` is called on the main thread after a successful operation.
pub fn copy_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    transfer_with_progress(
        source,
        dest_dir,
        parent_window,
        ignore_outcome(on_complete),
        false,
    );
}

/// Like `copy_with_progress`; `on_done` is told whether the copy was made
/// (not skipped, cancelled or failed).
pub fn copy_with_outcome(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_done: impl FnOnce(bool) + 'static,
) {
    transfer_with_progress(
        source,
        dest_dir,
        parent_window,
        Some(Box::new(on_done)),
        false,
    );
}

/// Called with whether a copy or move was made.
type TransferDone = Box<dyn FnOnce(bool) + 'static>;

fn ignore_outcome(on_complete: Option<Box<dyn FnOnce() + 'static>>) -> Option<TransferDone> {
    on_complete.map(|cb| Box::new(move |_| cb()) as TransferDone)
}

/// Move with a progress dialog.
/// `on_complete` is called on the main thread after a successful operation.
pub fn move_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    transfer_with_progress(
        source,
        dest_dir,
        parent_window,
        ignore_outcome(on_complete),
        true,
    );
}

fn transfer_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<TransferDone>,
    is_move: bool,
) {
    let Some(name) = source.basename() else {
        if let Some(cb) = on_complete {
            cb(false);
        }
        return;
    };
    let dest = dest_dir.child(&name);
    let Some(dest_path_buf) = dest_dir.path() else {
        if let Some(cb) = on_complete {
            cb(false);
        }
        return;
    };
    let dest_path = dest_path_buf.to_string_lossy().to_string();
    let display_name = name.to_string_lossy().to_string();

    // A folder can't go inside itself, and moving something to where it
    // already is does nothing.
    if let (Some(src), Some(dst)) = (source.path(), dest.path()) {
        let refusal = match placement(&src, &dst) {
            Placement::IntoItself => Some(format!(
                "Cannot {} {display_name} into itself",
                if is_move { "move" } else { "copy" }
            )),
            Placement::Same if is_move => Some(format!("{display_name} is already here")),
            _ => None,
        };
        if let Some(msg) = refusal {
            parent_window.announce(&msg, AccessibleAnnouncementPriority::High);
            if let Some(cb) = on_complete {
                cb(false);
            }
            return;
        }
    }

    // Checking for a clash touches the destination, which may be on a slow
    // mount, so it happens off the main thread.
    let source = source.clone();
    let parent = parent_window.clone();
    let parent_slow = parent_window.clone();
    let waiting = crate::background::waiting_message(&dest_path);
    let dest_for_check = dest.clone();
    crate::background::run(
        move || dest_for_check.query_exists(gio::Cancellable::NONE),
        move |exists| {
            if exists {
                let dialog_name = display_name.clone();
                let dialog_dest = dest.clone();
                let parent_for_choice = parent.clone();
                show_conflict_dialog(
                    &dialog_name,
                    &dialog_dest,
                    &parent,
                    move |choice| match choice {
                        ConflictChoice::Skip => {
                            parent_for_choice
                                .announce("Skipped", AccessibleAnnouncementPriority::Medium);
                            if let Some(cb) = on_complete {
                                cb(false);
                            }
                        }
                        ConflictChoice::Replace
                            if source.path().zip(dest.path()).is_some_and(|(s, d)| {
                                placement(&s, &d) != Placement::Elsewhere
                            }) =>
                        {
                            // Replacing would delete the source itself (or
                            // the folder holding it) before copying it.
                            parent_for_choice.announce(
                                &format!("Cannot replace {display_name} with itself"),
                                AccessibleAnnouncementPriority::High,
                            );
                            if let Some(cb) = on_complete {
                                cb(false);
                            }
                        }
                        ConflictChoice::Replace => {
                            run_with_progress(
                                ProgressOp {
                                    source,
                                    dest,
                                    display_name,
                                    dest_path,
                                    is_move,
                                    replace: true,
                                },
                                &parent_for_choice,
                                on_complete,
                            );
                        }
                        ConflictChoice::Rename(renamed) => {
                            run_with_progress(
                                ProgressOp {
                                    source,
                                    dest: renamed,
                                    display_name,
                                    dest_path,
                                    is_move,
                                    replace: false,
                                },
                                &parent_for_choice,
                                on_complete,
                            );
                        }
                    },
                );
            } else {
                run_with_progress(
                    ProgressOp {
                        source,
                        dest,
                        display_name,
                        dest_path,
                        is_move,
                        replace: false,
                    },
                    &parent,
                    on_complete,
                );
            }
        },
        move || {
            if let Some(msg) = waiting {
                parent_slow.announce(&msg, AccessibleAnnouncementPriority::Medium);
            }
        },
    );
}

/// Where a copy or move would put something, relative to itself.
#[derive(Debug, PartialEq)]
pub enum Placement {
    /// Onto itself (pasted into the folder it's in).
    Same,
    /// Inside itself: a folder into one of its own subfolders.
    IntoItself,
    /// Onto a folder that holds it, which replacing would delete.
    OverItsParent,
    Elsewhere,
}

/// Where `dest` is relative to `source` (by path; no filesystem access).
pub fn placement(source: &std::path::Path, dest: &std::path::Path) -> Placement {
    if dest == source {
        Placement::Same
    } else if dest.starts_with(source) {
        Placement::IntoItself
    } else if source.starts_with(dest) {
        Placement::OverItsParent
    } else {
        Placement::Elsewhere
    }
}

/// Why copying or moving `src` to `dst` must not go ahead, judged on the
/// real paths (links resolved): into itself, or replacing itself or the
/// folder that holds it (which would delete it first).
fn real_placement_refusal(src: &str, dst: &str, is_move: bool, replace: bool) -> Option<String> {
    let real_src = real_location(std::path::Path::new(src))?;
    let real_dst = real_location(std::path::Path::new(dst))?;
    match placement(&real_src, &real_dst) {
        Placement::IntoItself => Some(format!(
            "it can't be {} into itself",
            if is_move { "moved" } else { "copied" }
        )),
        Placement::Same | Placement::OverItsParent if replace => {
            Some("it can't replace itself".to_string())
        }
        _ => None,
    }
}

/// Recursively delete a directory using a cancellable (for cross-device move fallback).
fn delete_recursive_gio(
    file: &gio::File,
    cancellable: &gio::Cancellable,
) -> Result<(), glib::Error> {
    // Replacing a folder, or a move's source after copying: never into a
    // drive mounted inside it.
    refuse_mounts_inside(file)?;
    delete_tree_cancellable(file, cancellable)
}

fn delete_tree_cancellable(
    file: &gio::File,
    cancellable: &gio::Cancellable,
) -> Result<(), glib::Error> {
    let file_type = file.query_file_type(
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    );
    if file_type == gio::FileType::Directory {
        let enumerator = file.enumerate_children(
            "standard::name,standard::type",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(cancellable),
        )?;
        while let Some(info) = enumerator.next_file(Some(cancellable))? {
            let child = file.child(info.name());
            delete_tree_cancellable(&child, cancellable)?;
        }
    }
    file.delete(Some(cancellable))?;
    Ok(())
}

/// Recursively copy a directory and its contents.
fn copy_directory_recursive(
    source: &gio::File,
    dest: &gio::File,
    cancellable: &gio::Cancellable,
    progress: &Arc<Mutex<(i64, i64)>>,
    replace: bool,
) -> Result<(), glib::Error> {
    // When replacing, delete the existing destination directory first so we
    // get a true replacement instead of a merge.
    if replace {
        let dest_type = dest.query_file_type(
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(cancellable),
        );
        if dest_type == gio::FileType::Directory {
            delete_recursive_gio(dest, cancellable)?;
        } else if dest_type == gio::FileType::SymbolicLink {
            // Replace the link itself, not what's in the folder it leads to.
            dest.delete(Some(cancellable))?;
        }
    }

    // Create the destination directory (ignore "already exists" for non-replace
    // cases, e.g. copying into an existing tree on FUSE mounts)
    match dest.make_directory_with_parents(Some(cancellable)) {
        Ok(()) => {}
        Err(e) if e.matches(gio::IOErrorEnum::Exists) => {}
        Err(e) => return Err(e),
    }

    let flags = if replace {
        gio::FileCopyFlags::OVERWRITE
    } else {
        gio::FileCopyFlags::NONE
    };

    let enumerator = source.enumerate_children(
        "standard::name,standard::type",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    )?;

    while let Some(info) = enumerator.next_file(Some(cancellable))? {
        if cancellable.is_cancelled() {
            return Err(glib::Error::new(
                gio::IOErrorEnum::Cancelled,
                "Operation cancelled",
            ));
        }

        let child_source = source.child(info.name());
        let child_dest = dest.child(info.name());

        if info.file_type() == gio::FileType::Directory {
            copy_directory_recursive(&child_source, &child_dest, cancellable, progress, replace)?;
        } else {
            copy_keeping_links(&child_source, &child_dest, flags, cancellable, progress)?;
        }
    }

    Ok(())
}

/// Copy a file — a link as a link. Where the destination can't hold links
/// (FAT drives, most cloud drives), a link to a file is copied as that
/// file instead. A link to a folder that can't be made fails the copy, as
/// any other error does: skipping it would let a move delete it.
fn copy_keeping_links(
    src: &gio::File,
    dst: &gio::File,
    flags: gio::FileCopyFlags,
    cancellable: &gio::Cancellable,
    progress: &Arc<Mutex<(i64, i64)>>,
) -> Result<(), glib::Error> {
    let copy = |flags: gio::FileCopyFlags| {
        let ps = progress.clone();
        src.copy(
            dst,
            flags,
            Some(cancellable),
            Some(&mut |current, total| {
                let mut state = ps.lock().unwrap();
                *state = (current, total);
            }),
        )
    };
    match copy(flags | gio::FileCopyFlags::NOFOLLOW_SYMLINKS) {
        // Refused to make a link (not, say, a full disk or a name taken).
        Err(e)
            if (e.matches(gio::IOErrorEnum::NotSupported)
                || e.matches(gio::IOErrorEnum::PermissionDenied))
                && src.query_file_type(
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    Some(cancellable),
                ) == gio::FileType::SymbolicLink
                && src.query_file_type(gio::FileQueryInfoFlags::NONE, Some(cancellable))
                    == gio::FileType::Regular =>
        {
            copy(flags)
        }
        other => other,
    }
}

struct ProgressOp {
    source: gio::File,
    dest: gio::File,
    display_name: String,
    dest_path: String,
    is_move: bool,
    replace: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_with_progress(
    op: ProgressOp,
    parent_window: &gtk::Window,
    on_complete: Option<TransferDone>,
) {
    let ProgressOp {
        source,
        dest,
        display_name,
        dest_path,
        is_move,
        replace,
    } = op;
    let op_name = if is_move { "Moving" } else { "Copying" };

    // Build progress dialog (non-modal so the user can continue working)
    let dlg = gtk::Window::builder()
        .title(format!("{op_name} {display_name}"))
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

    let title_label = gtk::Label::builder()
        .label(format!("{op_name} {display_name} to {dest_path}"))
        .xalign(0.0)
        .wrap(true)
        .build();
    title_label.update_property(&[gtk::accessible::Property::Label(&format!(
        "{op_name} {display_name} to {dest_path}"
    ))]);

    let progress_bar = gtk::ProgressBar::new();
    progress_bar.set_show_text(true);
    progress_bar.update_property(&[gtk::accessible::Property::Label("Progress")]);

    // Status as a read-only entry so Orca users can tab to it and read it
    let status_entry = gtk::Entry::builder()
        .editable(false)
        .can_focus(true)
        .text("Preparing...")
        .build();
    status_entry.update_property(&[
        gtk::accessible::Property::Label("Transfer status"),
        gtk::accessible::Property::Description("Current progress of the file operation"),
    ]);

    let cancel_button = gtk::Button::with_label("Cancel");
    cancel_button.update_property(&[gtk::accessible::Property::Label("Cancel transfer")]);

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    button_box.set_halign(gtk::Align::End);
    button_box.append(&cancel_button);

    vbox.append(&title_label);
    vbox.append(&progress_bar);
    vbox.append(&status_entry);
    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));

    let cancellable = gio::Cancellable::new();

    let c = cancellable.clone();
    let d = dlg.clone();
    cancel_button.connect_clicked(move |_| {
        c.cancel();
        d.close();
    });

    dlg.present();
    status_entry.grab_focus();

    // Thread-safe shared progress state: (current_bytes, total_bytes)
    let progress_state = Arc::new(Mutex::new((0i64, 0i64)));
    let start_time = Instant::now();
    let tick_active = Arc::new(AtomicBool::new(true));

    // Timer to poll progress and update UI
    let ps_poll = progress_state.clone();
    let pb = progress_bar.clone();
    let se = status_entry.clone();
    let tick_active_poll = tick_active.clone();

    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        if !tick_active_poll.load(Ordering::Relaxed) {
            return glib::ControlFlow::Break;
        }
        let (current, total) = *ps_poll.lock().unwrap();
        if total > 0 {
            let fraction = current as f64 / total as f64;
            pb.set_fraction(fraction);

            let elapsed = start_time.elapsed().as_secs_f64();
            let remaining = if fraction > 0.01 {
                let total_est = elapsed / fraction;
                total_est - elapsed
            } else {
                0.0
            };

            let status_text = format!(
                "{} of {}, {}",
                format_size(current as u64),
                format_size(total as u64),
                format_time_remaining(remaining),
            );
            pb.set_text(Some(&format!("{:.0}%", fraction * 100.0)));
            se.set_text(&status_text);
        }
        glib::ControlFlow::Continue
    });

    // Run the copy/move in a real OS thread, communicate result via channel
    let ps = progress_state.clone();
    let Some(source_path_buf) = source.path() else {
        dlg.close();
        if let Some(cb) = on_complete {
            cb(false);
        }
        return;
    };
    let source_path = source_path_buf.to_string_lossy().to_string();
    let Some(dest_path_buf) = dest.path() else {
        dlg.close();
        if let Some(cb) = on_complete {
            cb(false);
        }
        return;
    };
    let dest_path_str = dest_path_buf.to_string_lossy().to_string();
    let cancellable_clone = cancellable.clone();

    // Channel for thread -> main thread result
    // Err carries whether it was a cancellation, and the message.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), (bool, String)>>();
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_flag_thread = done_flag.clone();

    std::thread::spawn(move || {
        let src = gio::File::for_path(&source_path);
        let dst = gio::File::for_path(&dest_path_str);

        let flags = if replace {
            gio::FileCopyFlags::OVERWRITE
        } else {
            gio::FileCopyFlags::NONE
        };

        // Checked before on the paths as shown; again here on real paths,
        // as a folder reached through a link is the same folder.
        let refusal = real_placement_refusal(&source_path, &dest_path_str, is_move, replace);

        let result = if let Some(msg) = refusal {
            Err(glib::Error::new(gio::IOErrorEnum::InvalidArgument, &msg))
        } else if is_move {
            let move_result = src.move_(
                &dst,
                flags,
                Some(&cancellable_clone),
                Some(&mut |current, total| {
                    let mut state = ps.lock().unwrap();
                    *state = (current, total);
                }),
            );
            // Cross-device directory move: GIO can't move directories across
            // filesystems (e.g. local → rclone FUSE mount). Fall back to
            // recursive copy + delete source.
            match move_result {
                Err(ref e)
                    if e.matches(gio::IOErrorEnum::NotSupported)
                        || e.matches(gio::IOErrorEnum::WouldMerge)
                        || e.matches(gio::IOErrorEnum::WouldRecurse) =>
                {
                    let file_type = src.query_file_type(
                        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                        gio::Cancellable::NONE,
                    );
                    if file_type == gio::FileType::Directory {
                        // Before copying: a drive mounted inside would be
                        // copied whole, only for deleting it to be refused.
                        refuse_mounts_inside(&src)
                            .and_then(|()| {
                                copy_directory_recursive(
                                    &src,
                                    &dst,
                                    &cancellable_clone,
                                    &ps,
                                    replace,
                                )
                            })
                            .and_then(|()| delete_recursive_gio(&src, &cancellable_clone))
                    } else {
                        copy_keeping_links(&src, &dst, flags, &cancellable_clone, &ps)
                            .and_then(|()| src.delete(Some(&cancellable_clone)))
                    }
                }
                other => other,
            }
        } else {
            let file_type = src.query_file_type(
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            );
            if file_type == gio::FileType::Directory {
                copy_directory_recursive(&src, &dst, &cancellable_clone, &ps, replace)
            } else {
                copy_keeping_links(&src, &dst, flags, &cancellable_clone, &ps)
            }
        };

        let _ =
            tx.send(result.map_err(|e| (e.matches(gio::IOErrorEnum::Cancelled), e.to_string())));
        done_flag_thread.store(true, Ordering::Relaxed);
    });

    // Poll for completion from the main thread
    let op_name_str = if is_move { "Moved" } else { "Copied" };
    let parent_window = parent_window.clone();
    // The name it now has ("Rename (keep both)" gives it a new one).
    let display_name_done = dest_path_buf
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| display_name.clone());
    let on_complete = std::cell::RefCell::new(on_complete);

    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if !done_flag.load(Ordering::Relaxed) {
            return glib::ControlFlow::Continue;
        }

        tick_active.store(false, Ordering::Relaxed);
        dlg.close();

        if let Ok(result) = rx.try_recv() {
            match result {
                Ok(()) => {
                    parent_window.announce(
                        &format!("{op_name_str} {display_name_done}"),
                        AccessibleAnnouncementPriority::Medium,
                    );
                    if let Some(cb) = on_complete.borrow_mut().take() {
                        cb(true);
                    }
                }
                Err((true, _)) => {
                    parent_window
                        .announce("Transfer cancelled", AccessibleAnnouncementPriority::Medium);
                    if let Some(cb) = on_complete.borrow_mut().take() {
                        cb(false);
                    }
                }
                Err((false, e)) => {
                    parent_window.announce(
                        &format!("Failed: {e}"),
                        AccessibleAnnouncementPriority::High,
                    );
                    if let Some(cb) = on_complete.borrow_mut().take() {
                        cb(false);
                    }
                }
            }
        }

        glib::ControlFlow::Break
    });
}

fn format_size(bytes: u64) -> String {
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

fn format_time_remaining(seconds: f64) -> String {
    if seconds <= 0.0 || seconds.is_nan() || seconds.is_infinite() {
        return "calculating...".to_string();
    }
    let secs = seconds as u64;
    if secs < 60 {
        if secs <= 1 {
            "less than 1 second".to_string()
        } else {
            format!("about {secs} seconds")
        }
    } else if secs < 3600 {
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        if remaining_secs == 0 {
            format!("about {mins} minute(s)")
        } else {
            format!("about {mins} minute(s) {remaining_secs} seconds")
        }
    } else {
        let hrs = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("about {hrs} hour(s) {mins} minute(s)")
    }
}

/// If dest already exists, append " (2)", " (3)", etc.
fn get_unique_dest(dest: &gio::File) -> gio::File {
    if !dest.query_exists(gio::Cancellable::NONE) {
        return dest.clone();
    }

    let Some(parent) = dest.parent() else {
        return dest.clone();
    };
    let Some(basename) = dest.basename() else {
        return dest.clone();
    };
    let name = basename.to_string_lossy();

    let (stem, ext) = if let Some(dot_pos) = name.rfind('.') {
        (&name[..dot_pos], Some(&name[dot_pos..]))
    } else {
        (name.as_ref(), None)
    };

    for i in 2..100 {
        let new_name = match ext {
            Some(ext) => format!("{stem} ({i}){ext}"),
            None => format!("{stem} ({i})"),
        };
        let candidate = parent.child(&new_name);
        if !candidate.query_exists(gio::Cancellable::NONE) {
            return candidate;
        }
    }

    dest.clone()
}

#[cfg(test)]
mod tests {
    use super::{copy_keeping_links, placement, Placement};
    use gtk::gio;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[test]
    fn links_are_copied_as_links() {
        let d = std::env::temp_dir().join(format!("wayfinder-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("folder")).unwrap();
        std::fs::write(d.join("file.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("folder", d.join("to-folder")).unwrap();
        let copy = |from: &str, to: &str| {
            copy_keeping_links(
                &gio::File::for_path(d.join(from)),
                &gio::File::for_path(d.join(to)),
                gio::FileCopyFlags::NONE,
                &gio::Cancellable::new(),
                &Arc::new(Mutex::new((0, 0))),
            )
        };
        copy("to-folder", "copied-link").unwrap();
        assert!(std::fs::symlink_metadata(d.join("copied-link"))
            .unwrap()
            .file_type()
            .is_symlink());
        copy("file.txt", "copied.txt").unwrap();
        assert_eq!(std::fs::read(d.join("copied.txt")).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pasting_into_its_own_folder_is_onto_itself() {
        assert_eq!(
            placement(Path::new("/x/Photos"), Path::new("/x/Photos")),
            Placement::Same
        );
    }

    #[test]
    fn a_folder_into_its_subfolder_is_into_itself() {
        assert_eq!(
            placement(Path::new("/u/a"), Path::new("/u/a/a")),
            Placement::IntoItself
        );
        assert_eq!(
            placement(Path::new("/u/a"), Path::new("/u/a/b/a")),
            Placement::IntoItself
        );
    }

    /// Cutting /x/a/a into /x clashes with /x/a, which holds the source.
    #[test]
    fn replacing_the_folder_that_holds_it() {
        assert_eq!(
            placement(Path::new("/x/a/a"), Path::new("/x/a")),
            Placement::OverItsParent
        );
    }

    /// Path components, not text: /u/ab isn't inside /u/a.
    /// The same folder reached through a link is still itself.
    #[test]
    fn replacing_itself_through_a_link_is_refused() {
        let d = std::env::temp_dir().join(format!("wayfinder-self-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("data/Docs/Photos")).unwrap();
        std::os::unix::fs::symlink(d.join("data/Docs"), d.join("Docs")).unwrap();
        let src = d.join("data/Docs/Photos").to_string_lossy().to_string();
        let dst = d.join("Docs/Photos").to_string_lossy().to_string();
        assert!(super::real_placement_refusal(&src, &dst, false, true).is_some());
        assert!(super::real_placement_refusal(&src, &dst, false, false).is_none());
        let into = d.join("Docs/Photos/Photos").to_string_lossy().to_string();
        assert!(super::real_placement_refusal(&src, &into, false, false).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A link is itself: copying it into the folder it leads to is fine.
    #[test]
    fn a_link_is_judged_as_itself() {
        let d = std::env::temp_dir().join(format!("wayfinder-linkself-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("data/Docs/Archive")).unwrap();
        std::os::unix::fs::symlink(d.join("data/Docs"), d.join("Docs")).unwrap();
        let src = d.join("Docs").to_string_lossy().to_string();
        let dst = d
            .join("data/Docs/Archive/Docs")
            .to_string_lossy()
            .to_string();
        assert!(super::real_placement_refusal(&src, &dst, false, false).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn similar_names_are_elsewhere() {
        assert_eq!(
            placement(Path::new("/u/a"), Path::new("/u/ab/a")),
            Placement::Elsewhere
        );
        assert_eq!(
            placement(Path::new("/u/a"), Path::new("/v/a")),
            Placement::Elsewhere
        );
    }
}
