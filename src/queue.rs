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

    /// The row of the thread with `id`.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.threads.iter().position(|t| t.id == id)
    }

    /// Removes the thread at row `index`, keeping the cursor on the thread
    /// it was on, or on the same row number when that is the one removed,
    /// so the next thread slides into place.
    pub fn remove(&mut self, index: usize) -> Thread {
        let thread = self.threads.remove(index);
        if index < self.cursor {
            self.cursor -= 1;
        }
        self.cursor = self.cursor.min(self.last());
        thread
    }

    /// Replaces the thread at row `index`.
    pub fn set(&mut self, index: usize, thread: Thread) {
        self.threads[index] = thread;
    }

    /// Inserts `thread` where its matching email sorts by `receivedAt`,
    /// oldest first when `ascending`, after any that tie. The cursor keeps
    /// its row number.
    pub fn insert_sorted(&mut self, thread: Thread, ascending: bool) {
        let at = thread.hit().received_at;
        let index = self
            .threads
            .iter()
            .position(|t| {
                let other = t.hit().received_at;
                if ascending { other > at } else { other < at }
            })
            .unwrap_or(self.threads.len());
        self.threads.insert(index, thread);
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
