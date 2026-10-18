//! The queue's rows and cursor.

use crate::mail::Thread;

/// The threads of the current filter, in order, and the cursor row.
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

    /// Replaces the rows, keeping the cursor on the same row number where it
    /// still exists.
    pub fn replace(&mut self, threads: Vec<Thread>) {
        self.threads = threads;
        self.cursor = self.cursor.min(self.last());
    }

    /// The thread under the cursor.
    pub fn current(&self) -> Option<&Thread> {
        self.threads.get(self.cursor)
    }

    /// Removes the thread under the cursor, keeping the cursor on the same
    /// row number so the next thread slides into place.
    pub fn remove_current(&mut self) -> Option<Thread> {
        if self.cursor >= self.threads.len() {
            return None;
        }
        let thread = self.threads.remove(self.cursor);
        self.cursor = self.cursor.min(self.last());
        Some(thread)
    }

    /// Puts `thread` back at row `index`, or last if there are fewer rows,
    /// unless a thread with its id is already in the queue, and says
    /// whether it did. The cursor keeps its row number.
    pub fn restore(&mut self, index: usize, thread: Thread) -> bool {
        if self.threads.iter().any(|t| t.id == thread.id) {
            return false;
        }
        let index = index.min(self.threads.len());
        self.threads.insert(index, thread);
        true
    }

    pub fn down(&mut self, rows: usize) {
        self.cursor = self.cursor.saturating_add(rows).min(self.last());
    }

    pub fn up(&mut self, rows: usize) {
        self.cursor = self.cursor.saturating_sub(rows);
    }

    pub fn to_first(&mut self) {
        self.cursor = 0;
    }

    pub fn to_last(&mut self) {
        self.cursor = self.last();
    }

    /// Moves to `line`, counting from 1, or the last row if there are fewer.
    pub fn to_line(&mut self, line: usize) {
        self.cursor = line.saturating_sub(1).min(self.last());
    }

    fn last(&self) -> usize {
        self.threads.len().saturating_sub(1)
    }
}
