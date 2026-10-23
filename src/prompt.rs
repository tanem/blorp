//! The `/` prompt the queue and the mailbox list share: the text that
//! narrows what they list, typed on the bottom line. Pure.

/// A key at the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey {
    Char(char),
    Backspace,
    /// Enter: lock the text in.
    Accept,
    /// Esc: put the text back as it was before `/`.
    Cancel,
}

/// The text, and while the prompt is open, the text as it was before `/`.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    /// Empty when none is set.
    text: String,
    before: Option<String>,
}

impl Prompt {
    /// The text; empty when none is set.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text while the prompt is open.
    pub fn typing(&self) -> Option<&str> {
        self.before.as_ref().map(|_| self.text.as_str())
    }

    /// `/`: opens the prompt, pre-filled with the text.
    pub fn open(&mut self) {
        self.before = Some(self.text.clone());
    }

    /// Handles a key while the prompt is open: a character or Backspace
    /// changes the text, Enter closes the prompt with the text as typed,
    /// and Esc closes it with the text as it was before `/`.
    pub fn key(&mut self, key: PromptKey) {
        let Some(before) = &self.before else {
            return;
        };
        match key {
            PromptKey::Char(c) => self.text.push(c),
            PromptKey::Backspace => {
                self.text.pop();
            }
            PromptKey::Accept => self.before = None,
            PromptKey::Cancel => {
                self.text = before.clone();
                self.before = None;
            }
        }
    }

    /// Esc with the prompt closed: clears the text, and says whether there
    /// was any.
    pub fn clear(&mut self) -> bool {
        !std::mem::take(&mut self.text).is_empty()
    }
}
