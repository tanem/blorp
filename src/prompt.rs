//! A one-line prompt: its text and cursor, the editing keys, and a
//! session history walked with Up and Down. Pure.

/// A key typed at a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Left,
    Right,
    /// Home or Ctrl-a.
    Home,
    /// End or Ctrl-e.
    End,
    /// Ctrl-w: delete the word before the cursor.
    DeleteWord,
    /// Ctrl-u: clear the line.
    Clear,
    Up,
    Down,
    Enter,
    Esc,
}

/// The text typed at a prompt, the cursor in it, and where Up and Down
/// have walked the history to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    text: String,
    /// The cursor, in characters from the start.
    cursor: usize,
    walk: Option<Walk>,
}

/// A walk through the history: the text typed before it began, which
/// filters the entries, and the entry shown.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Walk {
    typed: String,
    index: usize,
}

impl Prompt {
    /// A prompt holding `text`, with the cursor at its end.
    pub fn new(text: &str) -> Self {
        Prompt {
            text: text.into(),
            cursor: text.chars().count(),
            walk: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor, in characters from the start.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Applies an editing key. Up and Down walk `history`, newest first,
    /// through the entries that start with the text typed before the walk
    /// began; Down past the newest puts that text back. Enter and Esc are
    /// for the caller and change nothing here.
    pub fn edit(&mut self, key: Key, history: &History) {
        match key {
            Key::Up => return self.older(history),
            Key::Down => return self.newer(history),
            Key::Enter | Key::Esc => return,
            _ => self.walk = None,
        }
        match key {
            Key::Char(c) => {
                let at = self.byte(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte(self.cursor);
                self.text.remove(at);
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.text.chars().count()),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.text.chars().count(),
            Key::DeleteWord => {
                let before: Vec<char> = self.text.chars().take(self.cursor).collect();
                let mut start = before.len();
                while start > 0 && before[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !before[start - 1].is_whitespace() {
                    start -= 1;
                }
                let (from, to) = (self.byte(start), self.byte(self.cursor));
                self.text.replace_range(from..to, "");
                self.cursor = start;
            }
            Key::Clear => *self = Prompt::new(""),
            _ => {}
        }
    }

    fn older(&mut self, history: &History) {
        let typed = match &self.walk {
            Some(walk) => walk.typed.clone(),
            None => self.text.clone(),
        };
        let end = self.walk.as_ref().map_or(history.0.len(), |w| w.index);
        let found = (0..end)
            .rev()
            .find(|&i| history.0[i].starts_with(&typed) && history.0[i] != self.text);
        if let Some(index) = found {
            self.show(&history.0[index]);
            self.walk = Some(Walk { typed, index });
        }
    }

    fn newer(&mut self, history: &History) {
        let Some(walk) = self.walk.take() else {
            return;
        };
        let found =
            (walk.index + 1..history.0.len()).find(|&i| history.0[i].starts_with(&walk.typed));
        match found {
            Some(index) => {
                self.show(&history.0[index]);
                self.walk = Some(Walk { index, ..walk });
            }
            None => self.show(&walk.typed),
        }
    }

    fn show(&mut self, text: &str) {
        self.text = text.into();
        self.cursor = text.chars().count();
    }

    /// The byte offset of character `index`.
    fn byte(&self, index: usize) -> usize {
        self.text
            .char_indices()
            .nth(index)
            .map_or(self.text.len(), |(at, _)| at)
    }
}

/// A prompt's history for the session, oldest first, each entry once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History(Vec<String>);

impl History {
    /// Adds `entry` as the newest, moving it there if it is already held.
    /// Blank entries are not kept.
    pub fn push(&mut self, entry: &str) {
        if entry.trim().is_empty() {
            return;
        }
        self.0.retain(|e| e != entry);
        self.0.push(entry.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `keys` into a prompt holding `text`, and shows the result with
    /// `|` at the cursor.
    fn edited(text: &str, keys: &[Key]) -> String {
        let mut prompt = Prompt::new(text);
        for &key in keys {
            prompt.edit(key, &History::default());
        }
        let mut shown: String = prompt.text().chars().take(prompt.cursor()).collect();
        shown.push('|');
        shown.extend(prompt.text().chars().skip(prompt.cursor()));
        shown
    }

    #[test]
    fn the_editing_keys_edit_the_line_at_the_cursor() {
        use Key::*;
        for (text, keys, result) in [
            ("", vec![Char('a'), Char('b')], "ab|"),
            ("ab", vec![Backspace], "a|"),
            ("", vec![Backspace], "|"),
            ("ab", vec![Left, Char('x')], "ax|b"),
            ("ab", vec![Left, Left, Left], "|ab"),
            ("ab", vec![Right], "ab|"),
            ("ab", vec![Home, Char('x')], "x|ab"),
            ("ab", vec![Home, End], "ab|"),
            ("is:unread from:ana", vec![DeleteWord], "is:unread |"),
            ("is:unread from:ana  ", vec![DeleteWord], "is:unread |"),
            (
                "is:unread from:ana",
                vec![Left, Left, Left, DeleteWord],
                "is:unread |ana",
            ),
            ("ab", vec![Home, DeleteWord], "|ab"),
            ("is:unread", vec![Clear], "|"),
            ("café", vec![Left, Backspace, Char('e')], "cae|é"),
            ("ab", vec![Enter, Esc], "ab|"),
        ] {
            assert_eq!(edited(text, &keys), result, "{text:?} {keys:?}");
        }
    }
}
