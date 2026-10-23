//! The queue's rows and cursor.

use crate::mail::Thread;

/// The threads of the mailbox shown, newest first, and the cursor row.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    threads: Vec<Thread>,
    cursor: usize,
}

impl Queue {
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    /// The cursor's row, counting from 0. It is 0 in an empty queue.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the rows with `threads`, the queue as Fastmail now has it.
    /// The cursor stays on its thread if that is still in the queue, and
    /// otherwise on the same row number, or the last row if there are
    /// fewer.
    pub fn replace(&mut self, threads: Vec<Thread>) {
        let current = self.current().map(|t| t.id.clone());
        self.threads = threads;
        if let Some(index) = current.and_then(|id| self.threads.iter().position(|t| t.id == id)) {
            self.cursor = index;
        }
        self.cursor = self.cursor.min(self.last());
    }

    /// Removes the thread under the cursor. The cursor stays on the same
    /// row number, so the next thread slides into place, or on the last row
    /// if that was the one removed.
    pub fn remove_current(&mut self) -> Option<Thread> {
        if self.cursor >= self.threads.len() {
            return None;
        }
        let thread = self.threads.remove(self.cursor);
        self.cursor = self.cursor.min(self.last());
        Some(thread)
    }

    /// Inserts `thread` where it sorts, newest first by the email that
    /// stands for it, after any that tie. The cursor keeps its row number.
    pub fn insert(&mut self, thread: Thread) {
        let at = thread.hit().received_at;
        let index = self
            .threads
            .iter()
            .position(|t| t.hit().received_at < at)
            .unwrap_or(self.threads.len());
        self.threads.insert(index, thread);
    }

    /// Sets whether each email with an id in `ids` is seen, in whichever
    /// row it is.
    pub fn set_seen(&mut self, ids: &[String], seen: bool) {
        for email in self.threads.iter_mut().flat_map(|t| t.emails.iter_mut()) {
            if ids.contains(&email.id) {
                email.seen = seen;
            }
        }
    }

    /// The thread under the cursor.
    pub fn current(&self) -> Option<&Thread> {
        self.threads.get(self.cursor)
    }

    /// Moves the cursor down a row, stopping at the last.
    pub fn down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.last());
    }

    /// Moves the cursor up a row, stopping at the first.
    pub fn up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn last(&self) -> usize {
        self.threads.len().saturating_sub(1)
    }
}
