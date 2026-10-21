//! The queue's rows and cursor.

use crate::mail::Thread;

/// The Inbox's threads, newest first, and the cursor row.
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
