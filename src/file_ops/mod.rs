use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

// Simple (non-progress) file operations

pub fn trash_file(file: &gio::File) -> Result<(), glib::Error> {
    file.trash(gio::Cancellable::NONE)?;
    Ok(())
}

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

pub fn delete_file_recursive(file: &gio::File) -> Result<(), glib::Error> {
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
            delete_file_recursive(&child)?;
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
            format!("Deleting {}", &paths[0])
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
                        format!(
                            "Failed: {basename}: {}",
                            error.unwrap_or_default()
                        )
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
        let choice = match result {
            Ok(2) => ConflictChoice::Replace,
            Ok(1) => ConflictChoice::Rename(get_unique_dest(&dest)),
            _ => ConflictChoice::Skip,
        };
        callback(choice);
    });
}

/// Copy or move with a progress dialog.
/// `on_complete` is called on the main thread after a successful operation.
pub fn copy_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let Some(name) = source.basename() else {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    let dest = dest_dir.child(&name);
    let Some(dest_path_buf) = dest_dir.path() else {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    let dest_path = dest_path_buf.to_string_lossy().to_string();
    let display_name = name.to_string_lossy().to_string();

    if dest.query_exists(gio::Cancellable::NONE) {
        let source = source.clone();
        let parent = parent_window.clone();
        let dialog_name = display_name.clone();
        let dialog_dest = dest.clone();
        show_conflict_dialog(
            &dialog_name,
            &dialog_dest,
            parent_window,
            move |choice| match choice {
                ConflictChoice::Skip => {
                    parent.announce("Skipped", AccessibleAnnouncementPriority::Medium);
                    if let Some(cb) = on_complete {
                        cb();
                    }
                }
                ConflictChoice::Replace => {
                    run_with_progress(
                        ProgressOp {
                            source,
                            dest,
                            display_name,
                            dest_path,
                            is_move: false,
                            replace: true,
                        },
                        &parent,
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
                            is_move: false,
                            replace: false,
                        },
                        &parent,
                        on_complete,
                    );
                }
            },
        );
    } else {
        run_with_progress(
            ProgressOp {
                source: source.clone(),
                dest,
                display_name,
                dest_path,
                is_move: false,
                replace: false,
            },
            parent_window,
            on_complete,
        );
    }
}

pub fn move_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let Some(name) = source.basename() else {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    let dest = dest_dir.child(&name);
    let Some(dest_path_buf) = dest_dir.path() else {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    let dest_path = dest_path_buf.to_string_lossy().to_string();
    let display_name = name.to_string_lossy().to_string();

    if dest.query_exists(gio::Cancellable::NONE) {
        let source = source.clone();
        let parent = parent_window.clone();
        let dialog_name = display_name.clone();
        let dialog_dest = dest.clone();
        show_conflict_dialog(
            &dialog_name,
            &dialog_dest,
            parent_window,
            move |choice| match choice {
                ConflictChoice::Skip => {
                    parent.announce("Skipped", AccessibleAnnouncementPriority::Medium);
                    if let Some(cb) = on_complete {
                        cb();
                    }
                }
                ConflictChoice::Replace => {
                    run_with_progress(
                        ProgressOp {
                            source,
                            dest,
                            display_name,
                            dest_path,
                            is_move: true,
                            replace: true,
                        },
                        &parent,
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
                            is_move: true,
                            replace: false,
                        },
                        &parent,
                        on_complete,
                    );
                }
            },
        );
    } else {
        run_with_progress(
            ProgressOp {
                source: source.clone(),
                dest,
                display_name,
                dest_path,
                is_move: true,
                replace: false,
            },
            parent_window,
            on_complete,
        );
    }
}

/// Recursively delete a directory using a cancellable (for cross-device move fallback).
fn delete_recursive_gio(
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
            delete_recursive_gio(&child, cancellable)?;
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
            let ps = progress.clone();
            child_source.copy(
                &child_dest,
                flags,
                Some(cancellable),
                Some(&mut |current, total| {
                    let mut state = ps.lock().unwrap();
                    *state = (current, total);
                }),
            )?;
        }
    }

    Ok(())
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
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
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
            cb();
        }
        return;
    };
    let source_path = source_path_buf.to_string_lossy().to_string();
    let Some(dest_path_buf) = dest.path() else {
        dlg.close();
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    let dest_path_str = dest_path_buf.to_string_lossy().to_string();
    let cancellable_clone = cancellable.clone();

    // Channel for thread -> main thread result
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
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

        let result = if is_move {
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
                        copy_directory_recursive(&src, &dst, &cancellable_clone, &ps, replace)
                            .and_then(|()| delete_recursive_gio(&src, &cancellable_clone))
                    } else {
                        src.copy(
                            &dst,
                            flags,
                            Some(&cancellable_clone),
                            Some(&mut |current, total| {
                                let mut state = ps.lock().unwrap();
                                *state = (current, total);
                            }),
                        )
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
                src.copy(
                    &dst,
                    flags,
                    Some(&cancellable_clone),
                    Some(&mut |current, total| {
                        let mut state = ps.lock().unwrap();
                        *state = (current, total);
                    }),
                )
            }
        };

        let _ = tx.send(result.map_err(|e| e.to_string()));
        done_flag_thread.store(true, Ordering::Relaxed);
    });

    // Poll for completion from the main thread
    let op_name_str = if is_move { "Moved" } else { "Copied" };
    let parent_window = parent_window.clone();
    let display_name_done = display_name.clone();
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
                        cb();
                    }
                }
                Err(e) if e.contains("cancelled") || e.contains("Cancelled") => {
                    parent_window
                        .announce("Transfer cancelled", AccessibleAnnouncementPriority::Medium);
                    if let Some(cb) = on_complete.borrow_mut().take() {
                        cb();
                    }
                }
                Err(e) => {
                    parent_window.announce(
                        &format!("Failed: {e}"),
                        AccessibleAnnouncementPriority::High,
                    );
                    if let Some(cb) = on_complete.borrow_mut().take() {
                        cb();
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
