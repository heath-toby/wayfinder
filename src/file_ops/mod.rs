use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

mod delete;
mod transfer;

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

/// An error if a drive is mounted inside something in the Bin (it was
/// moved there with its folder): deleting from the Bin would delete what's
/// on the drive.
pub fn refuse_mounts_in_bin() -> Result<(), glib::Error> {
    let home_bin = dirs::data_dir()
        .map(|d| d.join("Trash"))
        .map(|bin| std::fs::canonicalize(&bin).unwrap_or(bin));
    // The Bin also holds what was binned on other drives, in their own
    // `.Trash-<uid>` (or `.Trash/<uid>`) folders.
    let uid = crate::rclone_ops::current_uid();
    let mounts = crate::rclone_ops::mountpoints_inside("/");
    let found = mounts.iter().find(|m| {
        home_bin
            .as_ref()
            .is_some_and(|bin| std::path::Path::new(m).starts_with(bin))
            || uid.is_some_and(|uid| in_drive_bin(m, uid))
    });
    match found {
        Some(mount) => Err(glib::Error::new(
            gio::IOErrorEnum::Busy,
            &format!("{mount} is mounted inside something in the Bin; unmount it first"),
        )),
        None => Ok(()),
    }
}

/// Whether `mount` is inside a drive's Bin folder for user `uid`.
fn in_drive_bin(mount: &str, uid: u32) -> bool {
    mount.contains(&format!("/.Trash-{uid}/")) || mount.contains(&format!("/.Trash/{uid}/"))
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

    let dest = get_unique_dest(&dest)?;
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
    refuse_mounts_in_bin()?;
    let trash = gio::File::for_uri("trash:///");
    let enumerator = trash.enumerate_children(
        "standard::name",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;

    let mut count = 0u32;
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
        // Deleted as a whole: the Bin removes a top-level item (folder and
        // all) itself, and refuses to have one taken apart.
        let child = trash.child(info.name());
        if let Err(e) = child.delete(gio::Cancellable::NONE) {
            log::warn!("Failed to delete Bin item: {e}");
        } else {
            count += 1;
        }
    }

    Ok(count)
}

pub fn rename_file(file: &gio::File, new_name: &str) -> Result<gio::File, glib::Error> {
    let direct = file.set_display_name(new_name, gio::Cancellable::NONE);
    // Only the case changing, on a drive that ignores case (FAT, exFAT, a
    // Windows share): the new name is "taken" by the file itself, and GIO
    // refuses. Then through a name of its own.
    let old_name = file.basename().map(|n| n.to_string_lossy().to_string());
    let case_only = old_name
        .as_deref()
        .is_some_and(|old| is_case_only_change(old, new_name));
    // (The name "taken" by the file itself, not by another one beside it:
    // the folder holds nothing spelled exactly so. Inode numbers can't
    // tell — some such drives number a file differently by spelling.)
    let same_file = || {
        let Some(parent) = file
            .path()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        else {
            return false;
        };
        // (An entry that can't be read might be it: then not.)
        std::fs::read_dir(parent)
            .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
            .is_ok_and(|entries| {
                !entries
                    .iter()
                    .any(|e| e.file_name() == std::ffi::OsStr::new(new_name))
            })
    };
    match direct {
        Err(e) if case_only && e.matches(gio::IOErrorEnum::Exists) && same_file() => {
            // Short (a long name's case can change too) and of its own (one
            // left by an earlier failure doesn't stand in the way).
            static STEP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            // (The time too: a process number comes round again.)
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let step = format!(
                ".wayfinder-renaming-{}-{}-{nanos}",
                std::process::id(),
                STEP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let between = file.set_display_name(&step, gio::Cancellable::NONE)?;
            between
                .set_display_name(new_name, gio::Cancellable::NONE)
                .map_err(|e| {
                    // Put it back as it was; if even that fails, say where
                    // it is (hidden, by that name).
                    let back = old_name
                        .as_deref()
                        .map(|old| between.set_display_name(old, gio::Cancellable::NONE));
                    if matches!(back, Some(Ok(_))) {
                        e
                    } else {
                        glib::Error::new(
                            gio::IOErrorEnum::Failed,
                            &format!("{e} (the file is now named {step})"),
                        )
                    }
                })
        }
        other => other,
    }
}

pub use delete::{delete_with_progress, DeleteCompleteCallback};

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
    let parent = parent_window.clone();
    dialog.choose(Some(parent_window), gio::Cancellable::NONE, move |result| {
        match result {
            Ok(2) => callback(ConflictChoice::Replace),
            Ok(1) => {
                // Finding a free name probes the destination, which may be
                // on a slow mount.
                crate::background::run(
                    move || get_unique_dest(&dest),
                    move |renamed| match renamed {
                        Ok(renamed) => callback(ConflictChoice::Rename(renamed)),
                        Err(e) => {
                            parent.announce(
                                &format!("Couldn't keep both: {}", e.message()),
                                AccessibleAnnouncementPriority::High,
                            );
                            callback(ConflictChoice::Skip);
                        }
                    },
                    || {},
                );
            }
            _ => callback(ConflictChoice::Skip),
        }
    });
}

/// Copy `source` into `dest_dir`, with a progress window.
/// `on_complete` is called on the main thread after a successful copy.
pub fn copy_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    transfer::transfer(
        vec![source.clone()],
        dest_dir,
        parent_window,
        false,
        ignore_outcome(on_complete),
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
    transfer::transfer(
        vec![source.clone()],
        dest_dir,
        parent_window,
        false,
        Some(Box::new(on_done)),
    );
}

fn ignore_outcome(
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) -> Option<transfer::TransferDone> {
    on_complete.map(|cb| Box::new(move |_| cb()) as transfer::TransferDone)
}

/// Move `source` into `dest_dir`, with a progress window.
/// `on_complete` is called on the main thread after a successful move.
pub fn move_with_progress(
    source: &gio::File,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    transfer::transfer(
        vec![source.clone()],
        dest_dir,
        parent_window,
        true,
        ignore_outcome(on_complete),
    );
}

/// Copy (or move) all of `sources` into `dest_dir`, in one progress window
/// (each clash asked about first, in turn). `on_complete` runs when it's
/// over, however it went.
pub fn transfer_all(
    sources: Vec<gio::File>,
    dest_dir: &gio::File,
    parent_window: &gtk::Window,
    is_move: bool,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    transfer::transfer(
        sources,
        dest_dir,
        parent_window,
        is_move,
        ignore_outcome(on_complete),
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
/// `moving`: for a move (as copy, then delete), so everything about each
/// item is kept — dates and all — as a move would.
fn copy_directory_recursive(
    source: &gio::File,
    dest: &gio::File,
    cancellable: &gio::Cancellable,
    progress: &transfer::SharedTally,
    replace: bool,
    moving: bool,
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
        } else if dest_type != gio::FileType::Unknown {
            // A file (a folder is replacing it), or a link — the link
            // itself, not what's in the folder it leads to.
            dest.delete(Some(cancellable))?;
        }
    }

    // Create the destination directory (ignore "already exists" for non-replace
    // cases, e.g. copying into an existing tree on FUSE mounts)
    let created = match dest.make_directory_with_parents(Some(cancellable)) {
        Ok(()) => true,
        Err(e) if e.matches(gio::IOErrorEnum::Exists) => false,
        Err(e) => return Err(e),
    };

    let metadata = if moving {
        gio::FileCopyFlags::ALL_METADATA
    } else {
        gio::FileCopyFlags::NONE
    };
    let flags = if replace {
        gio::FileCopyFlags::OVERWRITE
    } else {
        gio::FileCopyFlags::NONE
    } | metadata;

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
            copy_directory_recursive(
                &child_source,
                &child_dest,
                cancellable,
                progress,
                replace,
                moving,
            )?;
        } else if info.file_type() == gio::FileType::Special {
            // A socket or pipe can't be copied (GIO refuses, which would
            // stop the whole folder partway): a copy leaves it out. A move
            // stops, saying which (deleting the source after would lose
            // it).
            if moving {
                return Err(glib::Error::new(
                    gio::IOErrorEnum::NotSupported,
                    &format!(
                        "{} is a special file (a socket, pipe or device), which can't be moved this way",
                        child_source.parse_name()
                    ),
                ));
            }
            log::warn!("Left out {}: not a file to copy", child_source.uri());
            progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .items_done += 1;
        } else {
            copy_keeping_links(&child_source, &child_dest, flags, cancellable, progress)?;
        }
    }

    // The folder's own permissions (a private one stays private), and for
    // a move its dates: set last, as writing into it changes them (and a
    // read-only one couldn't be written into). Only on a folder made here,
    // not one merged into. Not getting them is no reason to fail, as for
    // files.
    if created {
        if let Err(e) = source.copy_attributes(dest, metadata, Some(cancellable)) {
            log::warn!("Couldn't copy the attributes of {}: {e}", source.uri());
        }
    }
    progress
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .items_done += 1;
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
    progress: &transfer::SharedTally,
) -> Result<(), glib::Error> {
    // (The file's size, as the copy reports it: added to what's done when
    // it's done.)
    let size = std::cell::Cell::new(0u64);
    let copy = |flags: gio::FileCopyFlags| {
        src.copy(
            dst,
            flags,
            Some(cancellable),
            Some(&mut |current, total| {
                size.set(total.max(0) as u64);
                progress.lock().unwrap_or_else(|e| e.into_inner()).current = current.max(0) as u64;
            }),
        )
    };
    let result = match copy(flags | gio::FileCopyFlags::NOFOLLOW_SYMLINKS) {
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
    };
    if result.is_ok() {
        let mut t = progress.lock().unwrap_or_else(|e| e.into_inner());
        t.bytes_done += size.get();
        t.current = 0;
        t.items_done += 1;
    }
    result
}

/// Whether what's at the destination (`dst`) goes before a Replace with
/// `src` starts: when it's of the other kind (GIO won't put a folder over a
/// file or the other way round), and for a move of a folder over a folder
/// (then the move is a rename, not a copy of everything). Otherwise the
/// copy replaces it itself.
fn clear_before_replace(src: gio::FileType, dst: gio::FileType, is_move: bool) -> bool {
    let is_dir = |k| k == gio::FileType::Directory;
    dst != gio::FileType::Unknown && (is_dir(src) != is_dir(dst) || (is_move && is_dir(src)))
}

/// Whether renaming `old` to `new` changes only the case of its letters.
pub fn is_case_only_change(old: &str, new: &str) -> bool {
    old != new && old.to_lowercase() == new.to_lowercase()
}

/// Whether nothing is at `path` (a link leading nowhere is something):
/// `Ok(true)` only when it's not found. An error if it can't be told — a
/// name that can't be checked isn't taken to be free, or what's there
/// would be written over unasked.
pub fn name_free(path: &std::path::Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

/// Whether something is at `file` — a link too, even one leading nowhere
/// (`query_exists` follows links, so a broken one looks free). One that
/// can't be checked counts as there, as in `name_free`: what's there
/// mustn't be merged into or written over unasked.
fn exists_here(file: &gio::File) -> bool {
    taken(file).unwrap_or(true)
}

/// Whether something is at `file`; an error if that can't be told.
fn taken(file: &gio::File) -> Result<bool, glib::Error> {
    match file.query_info(
        "standard::type",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    ) {
        Ok(_) => Ok(true),
        Err(e) if e.matches(gio::IOErrorEnum::NotFound) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `name` numbered: `a (2).txt`. A leading dot isn't an extension's
/// (`.bashrc (2)`, still hidden).
pub fn numbered_name(name: &str, i: u32) -> String {
    match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{} ({i}){}", &name[..dot], &name[dot..]),
        None => format!("{name} ({i})"),
    }
}

/// If dest already exists, append " (2)", " (3)", etc. An error if no
/// free name is found — never the taken one, which would be merged into —
/// or, at the first, if a name can't be checked (saying why: a mount
/// erroring, or the name too long).
fn get_unique_dest(dest: &gio::File) -> Result<gio::File, glib::Error> {
    if !taken(dest)? {
        return Ok(dest.clone());
    }
    let none = || glib::Error::new(gio::IOErrorEnum::Exists, "No free name was found");
    let parent = dest.parent().ok_or_else(none)?;
    let name = dest
        .basename()
        .ok_or_else(none)?
        .to_string_lossy()
        .to_string();
    for i in 2..1000 {
        let candidate = parent.child(numbered_name(&name, i));
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(none())
}

#[cfg(test)]
mod tests {
    /// Keep both: the next free number, never the taken name; a name
    /// that can't be checked (too long with its number) is an error.
    #[test]
    fn keep_both_finds_a_free_number_or_says_why_not() {
        let d = std::env::temp_dir().join(format!("wayfinder-unique-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "").unwrap();
        std::fs::write(d.join("a (2).txt"), "").unwrap();
        let long = format!("{}.txt", "x".repeat(250));
        std::fs::write(d.join(&long), "").unwrap();
        let dir = gio::File::for_path(&d);
        let got = super::get_unique_dest(&dir.child("a.txt")).map(|f| f.path());
        let too_long = super::get_unique_dest(&dir.child(&long));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(got.unwrap(), Some(d.join("a (3).txt")));
        assert!(too_long.is_err());
    }

    #[test]
    fn a_folder_move_stops_at_a_pipe_and_a_copy_leaves_it_out() {
        let d = std::env::temp_dir().join(format!("wayfinder-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a"), "a").unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(d.join("src/pipe"))
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        let progress = transfer::SharedTally::default();
        let run = |dst: &str, moving: bool| {
            copy_directory_recursive(
                &gio::File::for_path(d.join("src")),
                &gio::File::for_path(d.join(dst)),
                &gio::Cancellable::new(),
                &progress,
                false,
                moving,
            )
        };
        let copied = run("copy", false);
        let moved = run("moved", true);
        let copy_has_a = d.join("copy/a").exists();
        let pipe_kept = d.join("src/pipe").symlink_metadata().is_ok();
        let _ = std::fs::remove_dir_all(&d);
        assert!(copied.is_ok() && copy_has_a);
        assert!(moved.is_err_and(|e| e.message().contains("pipe")));
        assert!(pipe_kept);
    }

    #[test]
    fn a_case_only_rename_never_goes_over_another_file() {
        let d = std::env::temp_dir().join(format!("wayfinder-clash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("foo"), "mine").unwrap();
        std::fs::write(d.join("Foo"), "other").unwrap();
        let result = rename_file(&gio::File::for_path(d.join("foo")), "Foo");
        let other = std::fs::read_to_string(d.join("Foo")).unwrap();
        let mine = std::fs::read_to_string(d.join("foo")).ok();
        let _ = std::fs::remove_dir_all(&d);
        assert!(result.is_err());
        assert_eq!(other, "other");
        assert_eq!(mine.as_deref(), Some("mine"));
    }

    #[test]
    fn a_long_name_changes_case_directly() {
        let d = std::env::temp_dir().join(format!("wayfinder-long-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let name = format!("{}.txt", "a".repeat(240));
        let upper = format!("A{}", &name[1..]);
        std::fs::write(d.join(&name), "x").unwrap();
        let result = rename_file(&gio::File::for_path(d.join(&name)), &upper);
        let _ = std::fs::remove_dir_all(&d);
        assert!(result.is_ok());
    }

    #[test]
    fn a_case_only_rename_goes_through() {
        assert!(is_case_only_change("report.txt", "Report.txt"));
        assert!(!is_case_only_change("report.txt", "report.txt"));
        assert!(!is_case_only_change("report.txt", "summary.txt"));
        let d = std::env::temp_dir().join(format!("wayfinder-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("report.txt"), "x").unwrap();
        let renamed =
            rename_file(&gio::File::for_path(d.join("report.txt")), "Report.txt").unwrap();
        assert_eq!(
            renamed.basename().unwrap(),
            std::path::PathBuf::from("Report.txt")
        );
        assert!(d.join("Report.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    use super::transfer;
    use super::{
        clear_before_replace, copy_directory_recursive, copy_keeping_links, in_drive_bin,
        is_case_only_change, numbered_name, placement, rename_file, Placement,
    };
    use gtk::gio;
    use gtk::gio::prelude::*;
    use std::path::Path;

    #[test]
    fn replace_clears_the_other_kind_and_a_moved_folders_target() {
        use gio::FileType::{Directory as D, Regular as F, Unknown as None_};
        assert!(clear_before_replace(D, F, false));
        assert!(clear_before_replace(F, D, false));
        assert!(clear_before_replace(D, D, true));
        assert!(!clear_before_replace(D, D, false));
        assert!(!clear_before_replace(F, F, true));
        assert!(!clear_before_replace(D, None_, true));
    }

    #[test]
    fn copied_and_moved_folders_keep_their_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("wayfinder-copydir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let src = d.join("Private");
        std::fs::create_dir_all(src.join("keys")).unwrap();
        std::fs::write(src.join("keys/id"), "secret").unwrap();
        std::fs::set_permissions(src.join("keys"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86400);
        std::fs::File::options()
            .write(true)
            .open(src.join("keys/id"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let progress = transfer::SharedTally::default();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let mtime = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap();
        for (name, moving) in [("copy", false), ("moved", true)] {
            let dst = d.join(name);
            copy_directory_recursive(
                &gio::File::for_path(&src),
                &gio::File::for_path(&dst),
                &gio::Cancellable::new(),
                &progress,
                false,
                moving,
            )
            .unwrap();
            assert_eq!(mode(&dst.join("keys")), 0o700);
            assert_eq!(
                std::fs::read_to_string(dst.join("keys/id")).unwrap(),
                "secret"
            );
            if moving {
                // (To the microsecond: older GLib copies no finer.)
                let micros = |p: &std::path::Path| {
                    mtime(p)
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros()
                };
                assert_eq!(micros(&dst.join("keys/id")), micros(&src.join("keys/id")));
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn numbered_names_keep_extensions_and_hidden_files_hidden() {
        assert_eq!(numbered_name("a.txt", 2), "a (2).txt");
        assert_eq!(numbered_name("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc (2)");
        assert_eq!(numbered_name("README", 2), "README (2)");
    }

    #[test]
    fn mounts_in_a_drives_bin_are_found() {
        assert!(in_drive_bin(
            "/media/usb/.Trash-1000/files/Projects/remote",
            1000
        ));
        assert!(in_drive_bin("/media/usb/.Trash/1000/files/x", 1000));
        assert!(!in_drive_bin("/media/usb/.Trash-1001/files/x", 1000));
        assert!(!in_drive_bin("/media/usb/Projects/remote", 1000));
    }

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
                &transfer::SharedTally::default(),
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
