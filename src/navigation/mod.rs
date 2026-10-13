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

    pub fn go_up(&self) -> Option<PathBuf> {
        self.current().parent().map(|p| p.to_path_buf())
    }

    pub fn can_go_back(&self) -> bool {
        self.current_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.current_index + 1 < self.history.len()
    }
}
