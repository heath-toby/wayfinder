//! Conflicts a two-way sync left: a file changed in both places is kept
//! under its own name (the newer) and as `name.conflict-remote1` (or
//! `-local1`: the other). After a sync each is asked about — keep the
//! newer, both, or the older — and what isn't kept goes to the Bin.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file a sync found changed in both places.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// The file under its own name.
    pub original: PathBuf,
    /// The other version, beside it (`name.conflict-remote1`).
    pub copy: PathBuf,
    /// Whether the copy is the remote's version (otherwise this computer's).
    pub copy_from_remote: bool,
    pub original_modified: Option<SystemTime>,
    pub copy_modified: Option<SystemTime>,
}

/// What to keep of a conflict.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Keep {
    Newer,
    Both,
    Older,
}

/// For `name`, a conflict copy's name: the original's name, and whether
/// it's the remote's version. (As the sync names them, with the suffixes
/// `bisync_args` gives: `report.odt.conflict-remote1`.)
pub fn conflict_of(name: &str) -> Option<(String, bool)> {
    for (suffix, remote) in [(".conflict-remote", true), (".conflict-local", false)] {
        let Some(at) = name.rfind(suffix) else {
            continue;
        };
        let number = &name[at + suffix.len()..];
        if at > 0 && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
            return Some((name[..at].to_string(), remote));
        }
    }
    None
}

/// Every conflict under `root` (not following links), whose original is
/// still there. Reads the folder tree: off the main thread.
pub fn find_conflicts(root: &Path) -> Vec<Conflict> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some((original_name, copy_from_remote)) = conflict_of(&name) else {
                continue;
            };
            let original = folder.join(&original_name);
            let modified = |p: &Path| std::fs::symlink_metadata(p).ok()?.modified().ok();
            let Some(original_modified) = std::fs::symlink_metadata(&original)
                .ok()
                .map(|m| m.modified().ok())
            else {
                continue;
            };
            found.push(Conflict {
                copy_modified: modified(&path),
                original,
                copy: path,
                copy_from_remote,
                original_modified,
            });
        }
    }
    found.sort_by(|a, b| a.original.cmp(&b.original));
    found
}

/// The question about `conflict` (`remote`: the remote's name, "Proton"):
/// which copy is from where, and which is newer.
pub fn question(conflict: &Conflict, remote: &str) -> (String, String) {
    let name = conflict
        .original
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let here = "this computer".to_string();
    let there = remote.to_string();
    let (copy_from, original_from) = if conflict.copy_from_remote {
        (there, here)
    } else {
        (here, there)
    };
    let when = |t: Option<SystemTime>| {
        t.map(|t| {
            let t: chrono::DateTime<chrono::Local> = t.into();
            t.format("%-d %B at %H:%M").to_string()
        })
        .unwrap_or_else(|| "at an unknown time".to_string())
    };
    let message = format!("{name} was changed both here and on {remote}");
    let original = (original_from, conflict.original_modified);
    let copy = (copy_from, conflict.copy_modified);
    let detail = if conflict.original_modified == conflict.copy_modified {
        format!(
            "The copies from {} and from {} were both saved {}.",
            original.0,
            copy.0,
            when(original.1)
        )
    } else {
        let (newer, older) = if copy_is_newer(conflict) {
            (copy, original)
        } else {
            (original, copy)
        };
        format!(
            "The copy from {}, saved {}, is newer. The copy from {}, saved {}, is older.",
            newer.0,
            when(newer.1),
            older.0,
            when(older.1)
        )
    };
    (message, detail)
}

/// Whether the conflict copy is the newer version (otherwise the file
/// under its own name is, as the sync leaves it, or they're as new).
fn copy_is_newer(conflict: &Conflict) -> bool {
    conflict.copy_modified > conflict.original_modified
}

/// Do what was chosen for `conflict`: the version not kept goes to the Bin
/// (and the one kept takes the file's own name); keeping both gives the
/// copy a name of its own ("report (from Proton).odt"), so it isn't asked
/// about again. Touches the files: off the main thread.
pub fn resolve(conflict: &Conflict, keep: Keep, remote: &str) -> Result<(), String> {
    let copy_newer = copy_is_newer(conflict);
    let keep_copy = match keep {
        Keep::Both => {
            let from = if conflict.copy_from_remote {
                remote
            } else {
                "this computer"
            };
            let target = free_path(&both_name(&conflict.original, from));
            return std::fs::rename(&conflict.copy, target).map_err(|e| e.to_string());
        }
        Keep::Newer => copy_newer,
        Keep::Older => !copy_newer,
    };
    let to_bin = |p: &Path| {
        use gtk::gio::prelude::FileExt;
        gtk::gio::File::for_path(p)
            .trash(gtk::gio::Cancellable::NONE)
            .map_err(|e| e.to_string())
    };
    if keep_copy {
        to_bin(&conflict.original)?;
        std::fs::rename(&conflict.copy, &conflict.original).map_err(|e| e.to_string())
    } else {
        to_bin(&conflict.copy)
    }
}

/// The name a kept-both copy gets: "report (from Proton).odt".
fn both_name(original: &Path, from: &str) -> PathBuf {
    let name = original
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let named = match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{} (from {from}){}", &name[..dot], &name[dot..]),
        None => format!("{name} (from {from})"),
    };
    original.with_file_name(named)
}

/// `path`, or a numbered one beside it if that's taken.
fn free_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    std::iter::once(path.to_path_buf())
        .chain((2..1000).map(|i| path.with_file_name(crate::file_ops::numbered_name(&name, i))))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_copies_are_told_by_their_names() {
        assert_eq!(
            conflict_of("IIA Peters passage.odt.conflict-remote1"),
            Some(("IIA Peters passage.odt".to_string(), true))
        );
        assert_eq!(
            conflict_of("notes.conflict-local12"),
            Some(("notes".to_string(), false))
        );
        assert_eq!(conflict_of("a.conflict-remote"), None);
        assert_eq!(conflict_of("conflict-remote1"), None);
        assert_eq!(conflict_of("report.odt"), None);
    }

    fn at(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
    }

    #[test]
    fn the_question_says_which_is_newer_and_from_where() {
        let c = Conflict {
            original: PathBuf::from("/s/a.odt"),
            copy: PathBuf::from("/s/a.odt.conflict-remote1"),
            copy_from_remote: true,
            original_modified: at(2_000_000_000),
            copy_modified: at(1_999_996_400),
        };
        let (message, detail) = question(&c, "Proton");
        assert_eq!(message, "a.odt was changed both here and on Proton");
        assert!(
            detail.starts_with("The copy from this computer, saved "),
            "{detail}"
        );
        assert!(
            detail.contains("is newer. The copy from Proton, saved "),
            "{detail}"
        );
    }

    /// Each choice keeps the right version under the file's name (the other
    /// goes to the Bin — not tested here: it's the user's Bin), and keeping
    /// both gives the copy a name of its own.
    #[test]
    fn keeping_both_names_the_copy_after_where_it_came_from() {
        let d = std::env::temp_dir().join(format!("wayfinder-conflicts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/a.odt"), "mine").unwrap();
        std::fs::write(d.join("sub/a.odt.conflict-remote1"), "theirs").unwrap();
        std::fs::write(d.join("sub/gone.txt.conflict-local1"), "x").unwrap();
        let found = find_conflicts(&d);
        assert_eq!(found.len(), 1, "{found:?}");
        resolve(&found[0], Keep::Both, "Proton").unwrap();
        let kept = std::fs::read_to_string(d.join("sub/a (from Proton).odt"));
        let again = find_conflicts(&d);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(kept.unwrap(), "theirs");
        assert!(again.is_empty());
    }
}
