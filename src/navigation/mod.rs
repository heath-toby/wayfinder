use std::path::PathBuf;

pub struct NavigationState {
    history: Vec<PathBuf>,
    current_index: usize,
    /// The one entry is only where a window would start if nothing else is
    /// asked for: the first place gone to takes its place.
    seeded: bool,
}

impl NavigationState {
    pub fn new(initial: PathBuf) -> Self {
        Self {
            history: vec![initial],
            current_index: 0,
            seeded: false,
        }
    }

    /// Starting at `initial` unless somewhere else is gone to first (a
    /// window opened on a folder by another app shouldn't have its last
    /// session's folder to go Back to).
    pub fn seeded(initial: PathBuf) -> Self {
        Self {
            seeded: true,
            ..Self::new(initial)
        }
    }

    pub fn current(&self) -> &PathBuf {
        &self.history[self.current_index]
    }

    pub fn navigate_to(&mut self, path: PathBuf) {
        if std::mem::take(&mut self.seeded) {
            self.history[0] = path;
            return;
        }
        // Don't navigate to the same place
        if self.current() == &path {
            return;
        }
        // Truncate forward history
        self.history.truncate(self.current_index + 1);
        self.history.push(path);
        self.current_index += 1;
    }

    /// Replace the current entry, e.g. when the folder asked for turned out
    /// not to exist and its nearest existing parent was opened instead.
    pub fn replace_current(&mut self, path: PathBuf) {
        self.seeded = false;
        // The entry before may already be it (a missing folder opened its
        // parent, where we came from): one entry, or Back goes nowhere.
        if self.current_index > 0 && self.history[self.current_index - 1] == path {
            self.history.remove(self.current_index);
            self.current_index -= 1;
            return;
        }
        // Or the one after (Back to a missing folder opened the one we
        // left): one entry, or Forward goes nowhere.
        if self.history.get(self.current_index + 1) == Some(&path) {
            self.history.remove(self.current_index);
            return;
        }
        self.history[self.current_index] = path;
    }

    pub fn go_back(&mut self) -> Option<&PathBuf> {
        if self.can_go_back() {
            self.current_index -= 1;
            Some(&self.history[self.current_index])
        } else {
            None
        }
    }

    pub fn go_forward(&mut self) -> Option<&PathBuf> {
        if self.can_go_forward() {
            self.current_index += 1;
            Some(&self.history[self.current_index])
        } else {
            None
        }
    }

    /// The folder above the current one. None at the top, and for the Bin
    /// and Recent Files (`trash:///`, `recent:///`), which have no parent.
    pub fn go_up(&self) -> Option<PathBuf> {
        if self.current().to_string_lossy().contains("://") {
            return None;
        }
        self.current().parent().map(|p| p.to_path_buf())
    }

    pub fn can_go_back(&self) -> bool {
        self.current_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.current_index + 1 < self.history.len()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_window_sent_somewhere_first_has_no_start_to_go_back_to() {
        let mut nav = NavigationState::seeded(PathBuf::from("/mnt/cloud/Work"));
        nav.navigate_to(PathBuf::from("trash:///"));
        assert!(!nav.can_go_back());
        nav.navigate_to(PathBuf::from("/home/u"));
        assert!(nav.can_go_back());
        // Started where it was seeded: going on from there keeps it.
        let mut nav = NavigationState::seeded(PathBuf::from("/home/u"));
        nav.navigate_to(PathBuf::from("/home/u"));
        nav.navigate_to(PathBuf::from("/home/u/Docs"));
        assert!(nav.can_go_back());
    }

    use super::NavigationState;
    use std::path::PathBuf;

    /// /Docs → /Docs/gone, which opens /Docs: one entry, not two.
    #[test]
    fn a_missing_folder_opening_its_parent_isnt_a_step() {
        let mut nav = NavigationState::new(PathBuf::from("/Docs"));
        nav.navigate_to(PathBuf::from("/Docs/gone"));
        nav.replace_current(PathBuf::from("/Docs"));
        assert!(!nav.can_go_back());
        assert_eq!(nav.current(), &PathBuf::from("/Docs"));
    }

    /// /Docs/gone → /Docs, then Back to /Docs/gone, which opens /Docs:
    /// one entry, so Forward isn't left going to the same place.
    #[test]
    fn going_back_to_a_missing_folder_isnt_a_step_either() {
        let mut nav = NavigationState::new(PathBuf::from("/Docs/gone"));
        nav.navigate_to(PathBuf::from("/Docs"));
        nav.go_back();
        nav.replace_current(PathBuf::from("/Docs"));
        assert_eq!(nav.current(), &PathBuf::from("/Docs"));
        assert!(!nav.can_go_forward());
    }

    /// Home → Documents → Bin, then Back: Documents.
    #[test]
    fn back_from_the_bin_returns_to_the_folder_before_it() {
        let mut nav = NavigationState::new(PathBuf::from("/home/u"));
        nav.navigate_to(PathBuf::from("/home/u/Documents"));
        nav.navigate_to(PathBuf::from("trash:///"));
        assert_eq!(nav.go_up(), None);
        assert_eq!(nav.go_back(), Some(&PathBuf::from("/home/u/Documents")));
        assert_eq!(nav.go_forward(), Some(&PathBuf::from("trash:///")));
    }
}
