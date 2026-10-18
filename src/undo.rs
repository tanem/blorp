//! Undo and redo: the snapshot an action records, the linear history, and
//! the guarded patch an undo or redo sends. Pure.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::app::Action;
use crate::jmap::{self, EmailPatch, EmailUpdate};
use crate::mail::{self, Thread};

/// An email's mailboxes and the two keywords actions change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub mailbox_ids: BTreeSet<String>,
    pub seen: bool,
    pub flagged: bool,
}

impl State {
    /// The state after `patch`, given as `mailboxIds/<id>` and
    /// `keywords/<k>` paths set to `true` or `null`. Other keywords are
    /// ignored.
    pub fn patched(&self, patch: &EmailPatch) -> State {
        let mut state = self.clone();
        for (path, value) in patch {
            let on = *value == Value::Bool(true);
            match path.split_once('/') {
                Some(("mailboxIds", id)) if on => {
                    state.mailbox_ids.insert(id.to_string());
                }
                Some(("mailboxIds", id)) => {
                    state.mailbox_ids.remove(id);
                }
                Some(("keywords", "$seen")) => state.seen = on,
                Some(("keywords", "$flagged")) => state.flagged = on,
                _ => {}
            }
        }
        state
    }

    /// The patch that turns `self` into `to`, touching only what differs.
    pub fn patch_to(&self, to: &State) -> EmailPatch {
        let mut patch = EmailPatch::new();
        let set = |on: bool| if on { Value::Bool(true) } else { Value::Null };
        for id in self.mailbox_ids.difference(&to.mailbox_ids) {
            patch.insert(format!("mailboxIds/{id}"), Value::Null);
        }
        for id in to.mailbox_ids.difference(&self.mailbox_ids) {
            patch.insert(format!("mailboxIds/{id}"), Value::Bool(true));
        }
        if self.seen != to.seen {
            patch.insert("keywords/$seen".into(), set(to.seen));
        }
        if self.flagged != to.flagged {
            patch.insert("keywords/$flagged".into(), set(to.flagged));
        }
        patch
    }
}

impl From<&mail::Email> for State {
    fn from(email: &mail::Email) -> Self {
        State {
            mailbox_ids: email.mailbox_ids.clone(),
            seen: email.seen,
            flagged: email.flagged,
        }
    }
}

impl From<&jmap::Email> for State {
    fn from(email: &jmap::Email) -> Self {
        State {
            mailbox_ids: email.mailbox_ids.keys().cloned().collect(),
            seen: email.keywords.contains_key("$seen"),
            flagged: email.keywords.contains_key("$flagged"),
        }
    }
}

/// What an action did to one email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub id: String,
    pub before: State,
    pub after: State,
}

/// Which way an entry is applied: back to the state before the action, or
/// forward to the state it left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Before => Side::After,
            Side::After => Side::Before,
        }
    }
}

impl Change {
    pub fn state(&self, side: Side) -> &State {
        match side {
            Side::Before => &self.before,
            Side::After => &self.after,
        }
    }
}

/// One action on the history: the threads it touched as they were in
/// memory before it, and what it did to each email it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub action: Action,
    pub threads: Vec<Thread>,
    pub changes: Vec<Change>,
}

impl Entry {
    /// The job that moves every changed email to `side`, each only if it is
    /// still in the state the other side leaves it in.
    pub fn replay(&self, side: Side) -> Replay {
        let mut replay = Replay::default();
        for change in &self.changes {
            let from = change.state(side.other());
            replay.expect.insert(change.id.clone(), from.clone());
            replay
                .update
                .insert(change.id.clone(), from.patch_to(change.state(side)));
        }
        replay
    }

    /// How many of the entry's threads have an email among `ids`.
    pub fn threads_with(&self, ids: &[String]) -> usize {
        self.threads
            .iter()
            .filter(|t| t.emails.iter().any(|e| ids.contains(&e.id)))
            .count()
    }
}

/// Every action of the session that can be undone, and every undone one
/// that can be redone. Linear: a new action discards the redo entries.
#[derive(Debug, Default)]
pub struct History {
    done: Vec<Entry>,
    undone: Vec<Entry>,
    last_id: u64,
}

impl History {
    /// Pushes a new action and discards the redo entries.
    pub fn record(&mut self, action: Action, threads: Vec<Thread>, changes: Vec<Change>) -> &Entry {
        self.last_id += 1;
        self.undone.clear();
        self.done.push(Entry {
            id: self.last_id,
            action,
            threads,
            changes,
        });
        self.done.last().expect("just pushed")
    }

    /// Moves the latest action to the redo entries and returns it.
    pub fn undo(&mut self) -> Option<&Entry> {
        let entry = self.done.pop()?;
        self.undone.push(entry);
        self.undone.last()
    }

    /// Moves the latest undone action back to the undo entries and returns
    /// it.
    pub fn redo(&mut self) -> Option<&Entry> {
        let entry = self.undone.pop()?;
        self.done.push(entry);
        self.done.last()
    }

    /// Drops the entry with `id`, wherever it is.
    pub fn forget(&mut self, id: u64) {
        self.done.retain(|e| e.id != id);
        self.undone.retain(|e| e.id != id);
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }

    /// How many actions can be undone.
    pub fn depth(&self) -> usize {
        self.done.len()
    }

    /// True when there is nothing to undo or redo.
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.undone.is_empty()
    }
}

/// An undo or redo as one outbox job: `Email/get` on the ids of `expect`,
/// then an `Email/set` of `update` for those still in their expected state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replay {
    /// The state each email must still be in to be changed.
    pub expect: BTreeMap<String, State>,
    pub update: EmailUpdate,
}

impl Replay {
    pub fn ids(&self) -> Vec<String> {
        self.expect.keys().cloned().collect()
    }

    /// The part of `update` for the emails in `got` still as expected, and
    /// the ids of the rest: changed since, or gone.
    pub fn split(&self, got: &[jmap::Email]) -> (EmailUpdate, Vec<String>) {
        let mut update = EmailUpdate::new();
        let mut skipped = Vec::new();
        for (id, expected) in &self.expect {
            let current = got.iter().find(|e| &e.id == id).map(State::from);
            match (current, self.update.get(id)) {
                (Some(current), Some(patch)) if current == *expected => {
                    update.insert(id.clone(), patch.clone());
                }
                _ => skipped.push(id.clone()),
            }
        }
        (update, skipped)
    }
}

/// The result of a [`Replay`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replayed {
    /// The ids changed since, or gone, and so left alone.
    pub skipped: Vec<String>,
    /// The ids the `Email/set` did not update.
    pub not_updated: Vec<String>,
}
