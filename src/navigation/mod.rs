use std::path::PathBuf;

pub struct NavigationState {
    history: Vec<PathBuf>,
    current_index: usize,
}

impl NavigationState {
    pub fn new(initial: PathBuf) -> Self {
        Self {
            history: vec![initial],
            current_index: 0,
        }
    }

    pub fn current(&self) -> &PathBuf {
        &self.history[self.current_index]
    }

    pub fn navigate_to(&mut self, path: PathBuf) {
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
        // The entry before may already be it (a missing folder opened its
        // parent, where we came from): one entry, or Back goes nowhere.
        if self.current_index > 0 && self.history[self.current_index - 1] == path {
            self.history.remove(self.current_index);
            self.current_index -= 1;
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
