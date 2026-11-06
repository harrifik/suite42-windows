//! Snapshot Undo/Redo used by every editor, with a clean marker for the saved state.

pub struct History<T: Clone> {
    undo: Vec<(T, String)>,
    redo: Vec<(T, String)>,
    limit: usize,
    /// Distance from the saved state: 0 = clean. `None` when the saved state is unreachable.
    saved_offset: Option<i64>,
}

impl<T: Clone> Default for History<T> {
    fn default() -> Self {
        Self::new(100)
    }
}

impl<T: Clone> History<T> {
    pub fn new(limit: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), limit, saved_offset: Some(0) }
    }

    /// Records the state *before* a change.
    pub fn push(&mut self, before: T, label: impl Into<String>) {
        self.undo.push((before, label.into()));
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.saved_offset = match self.saved_offset {
            Some(o) if o >= 0 => Some(o + 1),
            _ => None,
        };
    }

    pub fn undo(&mut self, current: T) -> Option<T> {
        let (prev, label) = self.undo.pop()?;
        self.redo.push((current, label));
        self.saved_offset = self.saved_offset.map(|o| o - 1);
        Some(prev)
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        let (next, label) = self.redo.pop()?;
        self.undo.push((current, label));
        self.saved_offset = self.saved_offset.map(|o| o + 1);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(_, l)| l.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(_, l)| l.as_str())
    }

    pub fn mark_saved(&mut self) {
        self.saved_offset = Some(0);
    }

    pub fn mark_dirty(&mut self) {
        self.saved_offset = None;
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_offset != Some(0)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved_offset = Some(0);
    }
}
