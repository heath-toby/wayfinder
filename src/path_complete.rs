//! Completing a typed path, as a shell does: a unique match is filled in,
//! several are filled in as far as they agree, and `~` stands for the home
//! folder. A completion keeps the form it was typed in — `~/…` stays
//! `~/…`, an absolute path stays absolute — so it always extends the text.

use std::path::Path;

/// Replace a leading `~` (alone or before `/`) with the home folder.
pub fn expand_tilde(text: &str) -> String {
    expand_with(text, dirs::home_dir().as_deref())
}

/// Complete `partial`. `None` if nothing matches, or the matches agree on
/// nothing more than is typed. Lists a directory, so may block on a slow
/// mount: call it off the main thread.
pub fn complete(partial: &str) -> Option<String> {
    complete_with(partial, dirs::home_dir().as_deref())
}

/// What completing `typed` to `completed` added, to announce: the new
/// text, or the whole last name if completing changed its case.
pub fn added_text<'a>(typed: &str, completed: &'a str) -> &'a str {
    if let Some(added) = completed.strip_prefix(typed) {
        return added;
    }
    let trimmed = completed.trim_end_matches('/');
    let start = trimmed.rfind('/').map_or(0, |i| i + 1);
    &completed[start..]
}

/// The home folder as text, without a trailing `/` (`$HOME` may have one):
/// empty for `/` itself, so `~/x` becomes `/x`.
fn home_text(home: &Path) -> String {
    home.to_string_lossy().trim_end_matches('/').to_string()
}

fn expand_with(text: &str, home: Option<&Path>) -> String {
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            match format!("{}{rest}", home_text(home)) {
                // `~` alone, with home at `/`.
                expanded if expanded.is_empty() => "/".to_string(),
                expanded => expanded,
            }
        }
        _ => text.to_string(),
    }
}

/// Write `path` with `~` for the home folder, if it's in it.
fn contract_with(path: &str, home: &Path) -> String {
    let home = home_text(home);
    if path == "/" && home.is_empty() {
        return "~".to_string();
    }
    match path.strip_prefix(home.as_str()) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn complete_with(partial: &str, home: Option<&Path>) -> Option<String> {
    let expanded = expand_with(partial, home);
    // Answer in the form typed.
    let as_typed = |path: &str| match home {
        Some(home) if partial.starts_with('~') => contract_with(path, home),
        _ => path.to_string(),
    };

    let (dir_str, prefix) = if expanded.ends_with('/') {
        (expanded.as_str(), "")
    } else {
        let p = Path::new(&expanded);
        let dir = p.parent()?.to_str()?;
        let prefix = p.file_name()?.to_str()?;
        (dir, prefix)
    };

    let dir = if dir_str.is_empty() { "." } else { dir_str };
    let entries = std::fs::read_dir(dir).ok()?;

    let prefix_lower = prefix.to_lowercase();
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Skip hidden unless prefix starts with .
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            if !name.to_lowercase().starts_with(&prefix_lower) {
                return None;
            }
            let full = if dir_str.ends_with('/') || dir_str.is_empty() {
                format!("{dir_str}{name}")
            } else {
                format!("{dir_str}/{name}")
            };
            // Add trailing / for directories
            if e.file_type().ok()?.is_dir() {
                Some(format!("{full}/"))
            } else {
                Some(full)
            }
        })
        .collect();

    matches.sort();
    let (first, rest) = matches.split_first()?;
    if rest.is_empty() {
        return Some(as_typed(first));
    }

    let common = rest
        .iter()
        .fold(first.as_str(), |common, m| common_prefix(common, m));
    (common.len() > expanded.len()).then(|| as_typed(common))
}

/// The longest start `a` and `b` share, ending on a character boundary.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut end = 0;
    for ((i, x), y) in a.char_indices().zip(b.chars()) {
        if x != y {
            break;
        }
        end = i + x.len_utf8();
    }
    &a[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh directory holding `home/` (with the given subfolders and
    /// files) and `homex/`, a sibling whose name starts with home's.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, dirs: &[&str], files: &[&str]) -> Self {
            let root =
                std::env::temp_dir().join(format!("wayfinder-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("home")).unwrap();
            std::fs::create_dir_all(root.join("homex")).unwrap();
            for d in dirs {
                std::fs::create_dir_all(root.join("home").join(d)).unwrap();
            }
            for f in files {
                std::fs::write(root.join("home").join(f), b"").unwrap();
            }
            Self { root }
        }

        fn home(&self) -> PathBuf {
            self.root.join("home")
        }

        fn path(&self, rel: &str) -> String {
            format!("{}/{rel}", self.root.display())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn tilde_expands_only_alone_or_before_slash() {
        let home = Path::new("/home/toby");
        assert_eq!(expand_with("~", Some(home)), "/home/toby");
        assert_eq!(expand_with("~/Music", Some(home)), "/home/toby/Music");
        assert_eq!(expand_with("~bob/Music", Some(home)), "~bob/Music");
        assert_eq!(expand_with("/etc", Some(home)), "/etc");
        assert_eq!(expand_with("~/x", None), "~/x");
    }

    /// `$HOME` with a trailing slash mustn't double it, or a `~/…`
    /// completion would come back as an absolute path.
    #[test]
    fn home_with_trailing_slash() {
        let home = Path::new("/home/toby/");
        assert_eq!(expand_with("~/Music", Some(home)), "/home/toby/Music");
        assert_eq!(contract_with("/home/toby/Music/", home), "~/Music/");
        let f = Fixture::new("slash", &["Development"], &[]);
        let home = format!("{}/", f.home().display());
        let completed = complete_with("~/Dev", Some(Path::new(&home))).unwrap();
        assert_eq!(completed, "~/Development/");
    }

    #[test]
    fn home_at_root() {
        let home = Path::new("/");
        assert_eq!(expand_with("~", Some(home)), "/");
        assert_eq!(expand_with("~/Dev", Some(home)), "/Dev");
        assert_eq!(contract_with("/Development/", home), "~/Development/");
        assert_eq!(contract_with("/", home), "~");
    }

    #[test]
    fn tilde_contracts_only_at_a_folder_boundary() {
        let home = Path::new("/home/toby");
        assert_eq!(contract_with("/home/toby", home), "~");
        assert_eq!(contract_with("/home/toby/Music/", home), "~/Music/");
        assert_eq!(contract_with("/home/tobyx/", home), "/home/tobyx/");
    }

    /// The crash: an absolute path under home came back as `~/…`, shorter
    /// than what was typed, and slicing it by the typed length panicked.
    #[test]
    fn absolute_path_under_home_stays_absolute() {
        let f = Fixture::new("absolute", &["Development"], &[]);
        let typed = f.path("home/Dev");
        let completed = complete_with(&typed, Some(&f.home())).unwrap();
        assert_eq!(completed, f.path("home/Development/"));
        assert_eq!(added_text(&typed, &completed), "elopment/");
    }

    #[test]
    fn tilde_path_stays_tilde() {
        let f = Fixture::new("tilde", &["Development"], &[]);
        let completed = complete_with("~/Dev", Some(&f.home())).unwrap();
        assert_eq!(completed, "~/Development/");
        assert_eq!(added_text("~/Dev", &completed), "elopment/");
    }

    /// A sibling of home whose name starts with home's isn't written `~x`.
    #[test]
    fn sibling_of_home_is_not_contracted() {
        let f = Fixture::new("sibling", &[], &[]);
        let typed = format!("{}x", f.home().display());
        let completed = complete_with(&typed, Some(&f.home())).unwrap();
        assert_eq!(completed, f.path("homex/"));
    }

    #[test]
    fn several_matches_complete_to_what_they_share() {
        let f = Fixture::new("several", &[], &["report-2024.txt", "report-2025.txt"]);
        let completed = complete_with("~/rep", Some(&f.home())).unwrap();
        assert_eq!(completed, "~/report-202");
        // Nothing more to add.
        assert_eq!(complete_with("~/report-202", Some(&f.home())), None);
    }

    /// Names that differ inside a multi-byte character mustn't split it.
    #[test]
    fn shared_start_ends_on_a_character_boundary() {
        // The old code counted the shared start in characters and cut it
        // in bytes, landing inside the second "é" and panicking.
        let f = Fixture::new("utf8", &[], &["ééa", "ééb"]);
        let completed = complete_with("~/é", Some(&f.home())).unwrap();
        assert_eq!(completed, "~/éé");
    }

    #[test]
    fn match_in_another_case_announces_the_whole_name() {
        let f = Fixture::new("case", &["Documents"], &[]);
        let completed = complete_with("~/doc", Some(&f.home())).unwrap();
        assert_eq!(completed, "~/Documents/");
        assert_eq!(added_text("~/doc", &completed), "Documents/");
    }

    #[test]
    fn hidden_files_only_when_asked_for() {
        let f = Fixture::new("hidden", &[], &[".profile", "photo.jpg"]);
        assert_eq!(
            complete_with("~/p", Some(&f.home())).unwrap(),
            "~/photo.jpg"
        );
        assert_eq!(
            complete_with("~/.p", Some(&f.home())).unwrap(),
            "~/.profile"
        );
    }

    #[test]
    fn no_match_is_none() {
        let f = Fixture::new("none", &[], &["a.txt"]);
        assert_eq!(complete_with("~/zzz", Some(&f.home())), None);
    }
}
