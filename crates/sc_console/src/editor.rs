//! Input-box edit state: char buffer, cursor, history, and completion popup.
//!
//! Pure in-memory operations (no IO), unit-testable; the render layer only reads this state.

use crate::completion::CompletionItem;

/// Completion popup state.
#[derive(Clone, Debug, Default)]
pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    /// Currently highlighted index.
    pub selected: usize,
}

impl CompletionState {
    pub fn selected(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn select_prev(&mut self) {
        // Wrap from top to bottom (mirrors `select_next` wrapping bottom to top, keeping Up/Down predictable).
        if !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
        }
    }
}

/// Single-line editor (`buffer` stored per `char`, cursor as a char index, CJK safe).
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    buffer: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    /// History browse position: `None` means editing a fresh line; `Some(i)` points at `history[i]`.
    history_pos: Option<usize>,
    /// Draft of the fresh line before entering history (restored on exit).
    draft: Vec<char>,
    pub completion: Option<CompletionState>,
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len_chars(&self) -> usize {
        self.buffer.len()
    }

    pub fn clear_completion(&mut self) {
        self.completion = None;
    }

    fn changed(&mut self) {
        // Any edit action closes history browsing (draft semantics handled by the caller).
        self.history_pos = None;
        self.completion = None;
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += 1;
        self.changed();
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.buffer.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.changed();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
            self.changed();
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
            self.changed();
        }
    }

    pub fn delete_word_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = self.cursor;
        while pos > 0 && self.buffer[pos - 1].is_whitespace() {
            pos -= 1;
        }
        while pos > 0 && !self.buffer[pos - 1].is_whitespace() {
            pos -= 1;
        }
        self.buffer.drain(pos..self.cursor);
        self.cursor = pos;
        self.changed();
    }

    pub fn kill_to_end(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.truncate(self.cursor);
            self.changed();
        }
    }

    pub fn kill_to_start(&mut self) {
        if self.cursor > 0 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
            self.changed();
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.completion = None;
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
        self.completion = None;
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
        self.completion = None;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.buffer.len();
        self.completion = None;
    }

    /// Jump to a char index (out-of-range values clamp; used for mouse click positioning).
    pub fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.buffer.len());
        self.completion = None;
    }

    pub fn clear_line(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.changed();
    }

    /// Submit the current line: returns the text and records history (empty lines are skipped, returning `None`).
    pub fn submit(&mut self, max_history: usize) -> Option<String> {
        let text: String = self.buffer.iter().collect();
        let trimmed = text.trim().to_string();
        self.buffer.clear();
        self.cursor = 0;
        self.history_pos = None;
        self.draft.clear();
        self.completion = None;
        if trimmed.is_empty() {
            return None;
        }
        if self.history.last().map(|s| s != &trimmed).unwrap_or(true) {
            self.history.push(trimmed.clone());
            while self.history.len() > max_history.max(1) {
                self.history.remove(0);
            }
        }
        Some(trimmed)
    }

    /// Previous history entry (Up). Returns whether the switch happened.
    pub fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next = match self.history_pos {
            None => {
                self.draft = self.buffer.clone();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.history_pos = Some(next);
        self.buffer = self.history[next].chars().collect();
        self.cursor = self.buffer.len();
        self.completion = None;
        true
    }

    /// Next history entry (Down); returns true when returning to the draft.
    pub fn history_next(&mut self) -> bool {
        let Some(pos) = self.history_pos else {
            return false;
        };
        if pos + 1 >= self.history.len() {
            self.history_pos = None;
            self.buffer = std::mem::take(&mut self.draft);
            self.cursor = self.buffer.len();
        } else {
            self.history_pos = Some(pos + 1);
            self.buffer = self.history[pos + 1].chars().collect();
            self.cursor = self.buffer.len();
        }
        self.completion = None;
        true
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Replace the current token with a completion item (`token_start` is a char index).
    pub fn apply_completion(&mut self, token_start: usize, replace: &str, suffix_space: bool) {
        let start = token_start.min(self.cursor).min(self.buffer.len());
        self.buffer.drain(start..self.cursor);
        let mut insert: Vec<char> = replace.chars().collect();
        if suffix_space && !replace.ends_with(' ') {
            insert.push(' ');
        }
        for (i, c) in insert.iter().enumerate() {
            self.buffer.insert(start + i, *c);
        }
        self.cursor = start + insert.len();
        self.completion = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_and_cursor_moves() {
        let mut e = EditorState::new();
        e.insert_str("你好ab");
        assert_eq!(e.cursor(), 4);
        e.move_left();
        e.move_left();
        e.insert_char('X');
        assert_eq!(e.text(), "你好Xab");
        e.backspace();
        assert_eq!(e.text(), "你好ab");
    }

    #[test]
    fn word_ops_and_kill() {
        let mut e = EditorState::new();
        e.insert_str("say hello world");
        e.delete_word_before();
        assert_eq!(e.text(), "say hello ");
        e.kill_to_start();
        assert_eq!(e.text(), "");
        assert!(e.is_empty());
    }

    #[test]
    fn submit_records_history_without_dupes() {
        let mut e = EditorState::new();
        e.insert_str("stop");
        assert_eq!(e.submit(10), Some("stop".to_string()));
        e.insert_str("stop");
        assert_eq!(e.submit(10), Some("stop".to_string()));
        assert_eq!(e.history_len(), 1);
        e.insert_str("   ");
        assert_eq!(e.submit(10), None);
    }

    #[test]
    fn history_browse_restores_draft() {
        let mut e = EditorState::new();
        for cmd in ["a", "b"] {
            e.insert_str(cmd);
            e.submit(10);
        }
        e.insert_str("draft");
        assert!(e.history_prev());
        assert_eq!(e.text(), "b");
        assert!(e.history_prev());
        assert_eq!(e.text(), "a");
        assert!(e.history_next());
        assert_eq!(e.text(), "b");
        assert!(e.history_next());
        assert_eq!(e.text(), "draft");
    }

    #[test]
    fn apply_completion_replaces_token() {
        let mut e = EditorState::new();
        e.insert_str("st");
        e.apply_completion(0, "stop", true);
        assert_eq!(e.text(), "stop ");
    }
}
