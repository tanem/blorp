//! The queue's rows and cursor.

use crate::mail::Thread;

/// The threads of the mailbox shown, newest first, the search, and the
/// cursor row. The rows are the threads that match the search.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    threads: Vec<Thread>,
    /// The search, as typed; empty when none is set.
    search: String,
    cursor: usize,
}

impl Queue {
    /// The rows: the threads that match the search, newest first.
    pub fn rows(&self) -> Vec<&Thread> {
        self.threads
            .iter()
            .filter(|t| t.matches(&self.search))
            .collect()
    }

    /// The cursor's row, counting from 0. It is 0 in an empty queue.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The search, as typed; empty when none is set.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Whether the queue holds the thread with `id`, whether or not it
    /// matches the search.
    pub fn holds(&self, id: &str) -> bool {
        self.threads.iter().any(|t| t.id == id)
    }

    /// Sets the search to `search`. The cursor stays on its thread if that
    /// still matches, and otherwise goes to the first row.
    pub fn set_search(&mut self, search: String) {
        let current = self.current().map(|t| t.id.clone());
        self.search = search;
        self.cursor = current.and_then(|id| self.row_of(&id)).unwrap_or(0);
    }

    /// Replaces the threads with `threads`, the queue as Fastmail now has
    /// it. The cursor stays on its thread if that is still a row, and
    /// otherwise on the same row number, or the last row if there are
    /// fewer.
    pub fn replace(&mut self, threads: Vec<Thread>) {
        let current = self.current().map(|t| t.id.clone());
        self.threads = threads;
        if let Some(row) = current.and_then(|id| self.row_of(&id)) {
            self.cursor = row;
        }
        self.cursor = self.cursor.min(self.last());
    }

    /// Replaces the threads with a queue query's `threads`. The cursor
    /// stays on its thread if that is still a row, and otherwise goes to
    /// the first row.
    pub fn load(&mut self, threads: Vec<Thread>) {
        let current = self.current().map(|t| t.id.clone());
        self.threads = threads;
        self.cursor = current.and_then(|id| self.row_of(&id)).unwrap_or(0);
    }

    /// Removes the thread under the cursor. The cursor stays on the same
    /// row number, so the next row slides into place, or on the last row
    /// if that was the one removed.
    pub fn remove_current(&mut self) -> Option<Thread> {
        let index = self.index(self.cursor)?;
        let thread = self.threads.remove(index);
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
    /// thread it is.
    pub fn set_seen(&mut self, ids: &[String], seen: bool) {
        for thread in &mut self.threads {
            thread.set_seen(ids, seen);
        }
    }

    /// The thread under the cursor.
    pub fn current(&self) -> Option<&Thread> {
        self.index(self.cursor).map(|i| &self.threads[i])
    }

    /// Moves the cursor down a row, stopping at the last.
    pub fn down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.last());
    }

    /// Moves the cursor up a row, stopping at the first.
    pub fn up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// The index in `threads` of row `row`.
    fn index(&self, row: usize) -> Option<usize> {
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.matches(&self.search))
            .nth(row)
            .map(|(i, _)| i)
    }

    /// The row of the thread with `id`, if it is one.
    fn row_of(&self, id: &str) -> Option<usize> {
        self.rows().iter().position(|t| t.id == id)
    }

    fn last(&self) -> usize {
        self.rows().len().saturating_sub(1)
    }
}
