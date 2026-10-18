//! The queue's rows, cursor, marks, and visual range.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use crate::mail::Thread;

/// The threads of the current filter, in order, the cursor row, the marked
/// threads, and the visual range.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    threads: Vec<Thread>,
    cursor: usize,
    /// The ids of the marked threads.
    marks: BTreeSet<String>,
    /// The id of the thread the visual range was opened on, while it is
    /// open. The range runs from its row to the cursor.
    anchor: Option<String>,
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
    /// still exists, and the marks and visual range on threads still in the
    /// queue.
    pub fn replace(&mut self, threads: Vec<Thread>) {
        self.threads = threads;
        self.cursor = self.cursor.min(self.last());
        let threads = &self.threads;
        self.marks.retain(|id| threads.iter().any(|t| t.id == *id));
        if self
            .anchor
            .as_ref()
            .is_some_and(|id| self.index_of(id).is_none())
        {
            self.anchor = None;
        }
    }

    /// The thread under the cursor.
    pub fn current(&self) -> Option<&Thread> {
        self.threads.get(self.cursor)
    }

    /// The row of the thread with `id`.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.threads.iter().position(|t| t.id == id)
    }

    /// Removes the thread at row `index` and its mark, and closes the visual
    /// range if it was opened on that thread. The cursor stays on the
    /// thread it was on, or on the same row number when that is the one
    /// removed, so the next thread slides into place.
    pub fn remove(&mut self, index: usize) -> Thread {
        let thread = self.threads.remove(index);
        self.marks.remove(&thread.id);
        if self.anchor.as_ref() == Some(&thread.id) {
            self.anchor = None;
        }
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

    /// Whether the thread at row `index` is marked.
    pub fn marked(&self, index: usize) -> bool {
        self.threads
            .get(index)
            .is_some_and(|t| self.marks.contains(&t.id))
    }

    /// Marks each of `rows` rows from the cursor that is unmarked and
    /// unmarks each that is marked, then moves the cursor past them.
    pub fn toggle_marks(&mut self, rows: usize) {
        for index in self.cursor..self.threads.len().min(self.cursor.saturating_add(rows)) {
            let id = &self.threads[index].id;
            if !self.marks.remove(id) {
                self.marks.insert(id.clone());
            }
        }
        self.down(rows);
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    /// Opens a visual range on the row under the cursor.
    pub fn open_visual(&mut self) {
        self.anchor = self.current().map(|t| t.id.clone());
    }

    pub fn close_visual(&mut self) {
        self.anchor = None;
    }

    /// The rows of the visual range, while it is open.
    pub fn visual(&self) -> Option<RangeInclusive<usize>> {
        let anchor = self.index_of(self.anchor.as_ref()?)?;
        Some(anchor.min(self.cursor)..=anchor.max(self.cursor))
    }

    /// Clears the marks and closes the visual range.
    pub fn clear_selection(&mut self) {
        self.clear_marks();
        self.close_visual();
    }

    /// Whether the row at `index` is marked or in the visual range.
    pub fn selected(&self, index: usize) -> bool {
        self.marked(index) || self.visual().is_some_and(|range| range.contains(&index))
    }

    /// The rows an action applies to: the visual range if it is open, else
    /// the marked rows if there are any, else `count` rows from the cursor,
    /// or fewer at the end of the queue.
    pub fn selection(&self, count: usize) -> Vec<usize> {
        if let Some(range) = self.visual() {
            return range.collect();
        }
        if !self.marks.is_empty() {
            return (0..self.threads.len())
                .filter(|&i| self.marked(i))
                .collect();
        }
        (self.cursor..self.threads.len()).take(count).collect()
    }

    /// Moves to `line`, counting from 1, or the last row if there are fewer.
    pub fn to_line(&mut self, line: usize) {
        self.cursor = line.saturating_sub(1).min(self.last());
    }

    fn last(&self) -> usize {
        self.threads.len().saturating_sub(1)
    }
}
