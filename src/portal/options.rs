//! What an app asks of the file chooser beyond a title (file types, extra
//! choices, several files), and what the chooser remembers between uses.
//! Plain logic only: no widgets, so it can be tested without a display.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

/// One entry of the portal's `filters`: a name ("PDF documents") and its
/// rules — `(0, glob)` such as `*.pdf`, or `(1, mime type)` such as
/// `application/pdf` or `image/*`. Sent back as-is as `current_filter`.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub name: String,
    pub rules: Vec<(u32, String)>,
}

/// One entry of the portal's `choices`: an extra control the app wants
/// (an encoding, "Open read-only"). With no options it's a check box whose
/// value is "true" or "false"; otherwise a list of `(id, label)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// `filters` (`a(sa(us))`); none if absent or not understood. Filters with
/// no rules can't match anything and are left out.
pub fn filters(options: &HashMap<String, OwnedValue>) -> Vec<Filter> {
    options
        .get("filters")
        .and_then(|v| <Vec<(String, Vec<(u32, String)>)>>::try_from(Value::from(v.clone())).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, rules)| !rules.is_empty())
        .map(|(name, rules)| Filter { name, rules })
        .collect()
}

/// `current_filter` (`(sa(us))`).
pub fn current_filter(options: &HashMap<String, OwnedValue>) -> Option<Filter> {
    options
        .get("current_filter")
        .and_then(|v| <(String, Vec<(u32, String)>)>::try_from(Value::from(v.clone())).ok())
        .filter(|(_, rules)| !rules.is_empty())
        .map(|(name, rules)| Filter { name, rules })
}

/// `choices` (`a(ssa(ss)s)`).
pub fn choices(options: &HashMap<String, OwnedValue>) -> Vec<Choice> {
    type Raw = Vec<(String, String, Vec<(String, String)>, String)>;
    options
        .get("choices")
        .and_then(|v| <Raw>::try_from(Value::from(v.clone())).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(id, label, options, initial)| Choice {
            id,
            label,
            options,
            initial,
        })
        .collect()
}

/// `multiple`: whether several files may be chosen.
pub fn multiple(options: &HashMap<String, OwnedValue>) -> bool {
    options
        .get("multiple")
        .and_then(|v| <bool>::try_from(v.clone()).ok())
        .unwrap_or(false)
}

/// The filters to offer and which to start on: the app's list, with its
/// `current_filter` added if it isn't in it (as GTK does), starting on that
/// one, or else the first.
pub fn filter_list(mut filters: Vec<Filter>, current: Option<Filter>) -> (Vec<Filter>, usize) {
    let Some(current) = current else {
        return (filters, 0);
    };
    if let Some(i) = filters.iter().position(|f| *f == current) {
        return (filters, i);
    }
    if let Some(i) = filters.iter().position(|f| f.name == current.name) {
        return (filters, i);
    }
    filters.push(current);
    let last = filters.len() - 1;
    (filters, last)
}

/// Whether a file named `name`, of content type `content_type`, is one the
/// filter lets through. Globs ignore case: an app asking for `*.pdf` wants
/// `SCAN.PDF` too.
pub fn filter_matches(filter: &Filter, name: &str, content_type: &str) -> bool {
    filter.rules.iter().any(|(kind, pattern)| match kind {
        0 => glob_matches(pattern, name),
        1 => mime_matches(pattern, content_type),
        _ => false,
    })
}

fn mime_matches(pattern: &str, content_type: &str) -> bool {
    if matches!(pattern, "*" | "*/*") {
        return true;
    }
    if content_type.is_empty() {
        return false;
    }
    let mime = gtk::gio::content_type_get_mime_type(content_type)
        .map(|m| m.to_string())
        .unwrap_or_else(|| content_type.to_string());
    if let Some(major) = pattern.strip_suffix("/*") {
        return mime
            .split('/')
            .next()
            .is_some_and(|m| m.eq_ignore_ascii_case(major));
    }
    mime.eq_ignore_ascii_case(pattern) || gtk::gio::content_type_is_a(content_type, pattern)
}

/// Shell-style glob, ignoring case: `*`, `?`, `[abc]`, `[a-z]`, `[!x]`.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    glob_at(&p, &n)
}

fn glob_at(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => (0..=n.len()).any(|i| glob_at(&p[1..], &n[i..])),
        Some('?') => !n.is_empty() && glob_at(&p[1..], &n[1..]),
        Some('[') => {
            let Some(c) = n.first() else { return false };
            match bracket(&p[1..], *c) {
                Some((matched, rest)) => matched && glob_at(rest, &n[1..]),
                // No closing ]: a plain [.
                None => *c == '[' && glob_at(&p[1..], &n[1..]),
            }
        }
        Some(&c) => n.first() == Some(&c) && glob_at(&p[1..], &n[1..]),
    }
}

/// A `[...]` set (`p` starts after the `[`): whether `c` is in it, and the
/// pattern after the `]`.
fn bracket(p: &[char], c: char) -> Option<(bool, &[char])> {
    let (negate, mut i) = match p.first() {
        Some('!') | Some('^') => (true, 1),
        _ => (false, 0),
    };
    let mut found = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((found != negate, &p[i + 1..]));
        }
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if p[i] <= c && c <= p[i + 2] {
                found = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                found = true;
            }
            i += 1;
        }
    }
    None
}

/// The extension a filter saves with: its first plain `*.ext` glob (one
/// with no other wildcards). `*.[oO][dD][tT]` is read as `odt`.
pub fn filter_extension(filter: &Filter) -> Option<String> {
    filter.rules.iter().find_map(|(kind, pattern)| {
        if *kind != 0 {
            return None;
        }
        let rest = pattern.strip_prefix("*.")?;
        let ext = unbracket(rest)?;
        (!ext.is_empty()
            && ext
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .then_some(ext)
    })
}

/// `[oO][dD][tT]` → `odt`; plain text as it is; anything else (`*`, `?`,
/// a real set) → none.
fn unbracket(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '*' | '?' => return None,
            '[' => {
                let set: String = chars.by_ref().take_while(|&c| c != ']').collect();
                let mut letters = set.chars();
                let first = letters.next()?;
                if !letters.all(|c| c.eq_ignore_ascii_case(&first)) {
                    return None;
                }
                out.push(first.to_ascii_lowercase());
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// The file name to show when the save type changes from `from` to `to`:
/// the name's extension swapped for the new type's, if it had the old
/// type's (or none at all); `None` to leave the name alone.
pub fn swap_extension(name: &str, from: Option<&Filter>, to: &Filter) -> Option<String> {
    let new_ext = filter_extension(to)?;
    if name.is_empty() {
        return None;
    }
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => {
            // Only an extension the old type gave it: one the user typed
            // for another reason ("notes.v2") stays.
            let had_old = from.is_some_and(|f| {
                f.rules
                    .iter()
                    .any(|(k, p)| *k == 0 && glob_matches(p, name))
            });
            if !had_old {
                return None;
            }
            let swapped = format!("{stem}.{new_ext}");
            (swapped != name).then_some(swapped)
        }
        _ => Some(format!("{name}.{new_ext}")),
    }
}

/// The part of a file name before its extension, in characters: what's
/// selected when the name field is focused, so typing replaces the name
/// but keeps the type.
pub fn stem_chars(name: &str) -> i32 {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    i32::try_from(stem.chars().count()).unwrap_or(i32::MAX)
}

/// Names typed in the file name field: `"a.txt" "b.txt"` (as Windows shows
/// several), or one name as typed.
pub fn typed_names(text: &str) -> Vec<String> {
    let text = text.trim();
    if !text.starts_with('"') {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![text.to_string()]
        };
    }
    text.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// What the file name field shows for the files chosen: one name as it is,
/// several quoted.
pub fn names_text(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        many => many
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// What the chooser remembers between uses.
#[derive(Debug, Default, PartialEq)]
pub struct Memory {
    /// "More options" left open.
    pub expanded: bool,
    /// Folders saved to or opened from, most recent first.
    pub recent: Vec<String>,
}

pub const MAX_RECENT: usize = 10;

impl Memory {
    pub fn parse(text: &str) -> Self {
        let mut m = Memory::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("expanded", v)) => m.expanded = v.trim() == "true",
                Some(("recent", v))
                    if !v.is_empty()
                        && m.recent.len() < MAX_RECENT
                        && !m.recent.iter().any(|r| r == v) =>
                {
                    m.recent.push(v.to_string());
                }
                _ => {}
            }
        }
        m
    }

    pub fn format(&self) -> String {
        let mut s = format!("expanded={}\n", self.expanded);
        for r in &self.recent {
            s.push_str(&format!("recent={r}\n"));
        }
        s
    }

    /// `folder` to the front of the recent folders.
    pub fn used(&mut self, folder: &str) {
        self.recent.retain(|r| r != folder);
        self.recent.insert(0, folder.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    fn path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|d| d.join("wayfinder").join("portal-state"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// Saved again from disk with `change` applied, so two choosers open at
    /// once don't undo each other's changes.
    pub fn update(change: impl FnOnce(&mut Memory)) {
        let Some(path) = Self::path() else { return };
        let mut m = Self::load();
        change(&mut m);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, m.format()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Where Up/Down in the location goes among the recent folders: from the
/// one shown (if it's one of them) to the next or previous; from anything
/// else, Down starts at the most recent. `None` past either end.
pub fn recent_step(recent: &[String], shown: &str, down: bool) -> Option<usize> {
    let shown = shown.trim_end_matches('/');
    let at = recent.iter().position(|r| r.trim_end_matches('/') == shown);
    match (at, down) {
        (Some(i), true) => (i + 1 < recent.len()).then_some(i + 1),
        (Some(i), false) => i.checked_sub(1),
        (None, true) => (!recent.is_empty()).then_some(0),
        (None, false) => None,
    }
}

/// Type-ahead in the file list: the row to move to for the letters typed
/// so far (`typed`, any case), from the row at `current`. One letter, or
/// the same letter pressed again and again, moves on to the next name
/// starting with it (round to the top); more letters find the first name
/// starting with them, from the current row on. Case is ignored.
pub fn type_ahead(names: &[String], current: Option<usize>, typed: &str) -> Option<usize> {
    let typed = typed.to_lowercase();
    let first = typed.chars().next()?;
    let n = names.len();
    if n == 0 {
        return None;
    }
    let repeat = typed.chars().all(|c| c == first);
    let (prefix, start) = if repeat {
        (first.to_string(), current.map_or(0, |c| c + 1))
    } else {
        (typed, current.unwrap_or(0))
    };
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| names[i].to_lowercase().starts_with(&prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, globs: &[&str]) -> Filter {
        Filter {
            name: name.into(),
            rules: globs.iter().map(|g| (0, g.to_string())).collect(),
        }
    }

    #[test]
    fn typing_finds_names() {
        let names: Vec<String> = ["Apple", "apricot", "Banana", "band", "Cherry"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(type_ahead(&names, None, "b"), Some(2));
        assert_eq!(type_ahead(&names, Some(2), "ban"), Some(2));
        assert_eq!(type_ahead(&names, Some(2), "band"), Some(3));
        // The same letter again: on to the next, round to the top.
        assert_eq!(type_ahead(&names, Some(0), "aa"), Some(1));
        assert_eq!(type_ahead(&names, Some(1), "aaa"), Some(0));
        assert_eq!(type_ahead(&names, Some(4), "C"), Some(4));
        assert_eq!(type_ahead(&names, None, "z"), None);
        assert_eq!(type_ahead(&[], None, "a"), None);
        assert_eq!(type_ahead(&names, None, ""), None);
    }

    #[test]
    fn globs() {
        assert!(glob_matches("*.pdf", "Scan.PDF"));
        assert!(glob_matches("*.[oO][dD][tT]", "a.odt"));
        assert!(glob_matches("photo-??.jpg", "photo-01.jpg"));
        assert!(!glob_matches("photo-??.jpg", "photo-1.jpg"));
        assert!(glob_matches("[a-c]*", "beta"));
        assert!(!glob_matches("[!a-c]*", "beta"));
        assert!(glob_matches("*", ""));
        assert!(!glob_matches("*.pdf", "pdf"));
        assert!(glob_matches("a[", "a["));
    }

    #[test]
    fn mime_rules() {
        let images = Filter {
            name: "Images".into(),
            rules: vec![(1, "image/*".into())],
        };
        assert!(filter_matches(&images, "x", "image/png"));
        assert!(!filter_matches(&images, "x", "text/plain"));
        let all = Filter {
            name: "All".into(),
            rules: vec![(1, "*/*".into())],
        };
        assert!(filter_matches(&all, "x", ""));
    }

    #[test]
    fn extensions_follow_the_type() {
        let odt = f("ODF Text", &["*.odt"]);
        let docx = f("Word", &["*.[dD][oO][cC][xX]"]);
        let any = f("All files", &["*"]);
        assert_eq!(filter_extension(&docx).as_deref(), Some("docx"));
        assert_eq!(filter_extension(&any), None);
        assert_eq!(
            swap_extension("Letter.odt", Some(&odt), &docx).as_deref(),
            Some("Letter.docx")
        );
        assert_eq!(
            swap_extension("Letter", Some(&odt), &docx).as_deref(),
            Some("Letter.docx")
        );
        // An extension the old type didn't give stays.
        assert_eq!(swap_extension("notes.v2", Some(&odt), &docx), None);
        // To "All files": nothing to give it.
        assert_eq!(swap_extension("Letter.odt", Some(&odt), &any), None);
        assert_eq!(swap_extension("", Some(&odt), &docx), None);
        assert_eq!(
            swap_extension(".bashrc", Some(&odt), &docx).as_deref(),
            Some(".bashrc.docx")
        );
    }

    #[test]
    fn current_filter_is_offered() {
        let a = f("A", &["*.a"]);
        let b = f("B", &["*.b"]);
        assert_eq!(filter_list(vec![a.clone(), b.clone()], None).1, 0);
        assert_eq!(
            filter_list(vec![a.clone(), b.clone()], Some(b.clone())).1,
            1
        );
        let c = f("C", &["*.c"]);
        let (list, i) = filter_list(vec![a, b], Some(c.clone()));
        assert_eq!((list.len(), &list[i]), (3, &c));
    }

    #[test]
    fn names_typed_and_shown() {
        assert_eq!(typed_names("  report.pdf "), vec!["report.pdf"]);
        assert_eq!(
            typed_names("\"a b.txt\" \"c.txt\""),
            vec!["a b.txt", "c.txt"]
        );
        assert!(typed_names("  ").is_empty());
        let two = vec!["a.txt".to_string(), "b c.txt".to_string()];
        assert_eq!(names_text(&two), "\"a.txt\" \"b c.txt\"");
        assert_eq!(typed_names(&names_text(&two)), two);
        assert_eq!(names_text(&["x".to_string()]), "x");
        assert_eq!(stem_chars("Letter.odt"), 6);
        assert_eq!(stem_chars(".bashrc"), 7);
        assert_eq!(stem_chars("café"), 4);
    }

    #[test]
    fn memory_round_trip_and_recent_order() {
        let mut m = Memory::parse("expanded=true\nrecent=/a\nrecent=/b\nrecent=/a\njunk\n");
        assert_eq!(m.recent, vec!["/a", "/b"]);
        assert!(m.expanded);
        m.used("/b");
        assert_eq!(m.recent, vec!["/b", "/a"]);
        for i in 0..20 {
            m.used(&format!("/{i}"));
        }
        assert_eq!(m.recent.len(), MAX_RECENT);
        assert_eq!(Memory::parse(&m.format()), m);
        assert_eq!(Memory::parse(""), Memory::default());
    }

    #[test]
    fn stepping_through_recent_folders() {
        let r: Vec<String> = ["/a", "/b", "/c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(recent_step(&r, "/elsewhere", true), Some(0));
        assert_eq!(recent_step(&r, "/elsewhere", false), None);
        assert_eq!(recent_step(&r, "/a/", true), Some(1));
        assert_eq!(recent_step(&r, "/c", true), None);
        assert_eq!(recent_step(&r, "/b", false), Some(0));
        assert_eq!(recent_step(&[], "/a", true), None);
    }

    #[test]
    fn options_from_dbus() {
        let mut o: HashMap<String, OwnedValue> = HashMap::new();
        let filters = vec![
            (
                "PDF".to_string(),
                vec![
                    (0u32, "*.pdf".to_string()),
                    (1, "application/pdf".to_string()),
                ],
            ),
            ("Empty".to_string(), vec![]),
        ];
        o.insert(
            "filters".into(),
            OwnedValue::try_from(Value::from(filters)).unwrap(),
        );
        o.insert(
            "current_filter".into(),
            OwnedValue::try_from(Value::from((
                "PDF".to_string(),
                vec![(0u32, "*.pdf".to_string())],
            )))
            .unwrap(),
        );
        let choices = vec![(
            "encoding".to_string(),
            "Encoding".to_string(),
            vec![("utf8".to_string(), "Unicode".to_string())],
            "utf8".to_string(),
        )];
        o.insert(
            "choices".into(),
            OwnedValue::try_from(Value::from(choices)).unwrap(),
        );
        o.insert("multiple".into(), OwnedValue::from(true));
        let got = super::filters(&o);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rules.len(), 2);
        assert_eq!(current_filter(&o).unwrap().name, "PDF");
        let c = super::choices(&o);
        assert_eq!((c[0].id.as_str(), c[0].options.len()), ("encoding", 1));
        assert!(multiple(&o));
        assert!(super::filters(&HashMap::new()).is_empty());
    }
}
