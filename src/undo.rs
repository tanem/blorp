//! Undo and redo: the snapshot an action records, the linear history, and
//! the patches an undo or redo sends. Pure.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::app::Action;
use crate::jmap::{self, EmailPatch, EmailUpdate};
use crate::mail::{self, Thread};
use crate::snooze;

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
    /// The opposite side.
    pub fn other(self) -> Side {
        match self {
            Side::Before => Side::After,
            Side::After => Side::Before,
        }
    }
}

impl Change {
    /// The email's state on `side` of the action.
    pub fn state(&self, side: Side) -> &State {
        match side {
            Side::Before => &self.before,
            Side::After => &self.after,
        }
    }
}

/// What an action did to one thread's snooze schedule entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleChange {
    pub thread_id: String,
    pub before: Option<snooze::Entry>,
    pub after: Option<snooze::Entry>,
}

impl ScheduleChange {
    /// The thread's entry on `side` of the action.
    pub fn state(&self, side: Side) -> Option<&snooze::Entry> {
        match side {
            Side::Before => self.before.as_ref(),
            Side::After => self.after.as_ref(),
        }
    }
}

/// One action on the history: the threads it touched as they were in
/// memory before it, what it did to each email it changed, and what it did
/// to their snooze schedule entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub action: Action,
    pub threads: Vec<Thread>,
    pub changes: Vec<Change>,
    pub schedule: Vec<ScheduleChange>,
}

impl Entry {
    /// The undo job: each changed email back to its state before the
    /// action, if it is still as the action left it.
    pub fn undo(&self) -> Replay {
        let mut replay = Replay::default();
        for change in &self.changes {
            replay
                .expect
                .insert(change.id.clone(), change.after.clone());
            let patch = change.after.patch_to(&change.before);
            replay.update.insert(change.id.clone(), patch);
        }
        replay
    }

    /// The redo's `Email/set`: the action's patch for each changed email.
    pub fn redo(&self) -> EmailUpdate {
        self.changes
            .iter()
            .map(|c| (c.id.clone(), c.before.patch_to(&c.after)))
            .collect()
    }

    /// `thread` with each email the entry changed in its `side` state.
    pub fn thread_at(&self, thread: &Thread, side: Side) -> Thread {
        let mut thread = thread.clone();
        for email in &mut thread.emails {
            if let Some(change) = self.changes.iter().find(|c| c.id == email.id) {
                let state = change.state(side);
                email.mailbox_ids = state.mailbox_ids.clone();
                email.seen = state.seen;
                email.flagged = state.flagged;
            }
        }
        thread
    }

    /// The first mailbox an email of `thread` is in on `side` and not on
    /// the other.
    pub fn mailbox_gained(&self, thread: &Thread, side: Side) -> Option<String> {
        self.changes
            .iter()
            .filter(|c| thread.emails.iter().any(|e| e.id == c.id))
            .find_map(|c| {
                let to = &c.state(side).mailbox_ids;
                to.difference(&c.state(side.other()).mailbox_ids)
                    .next()
                    .cloned()
            })
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
    pub fn record(
        &mut self,
        action: Action,
        threads: Vec<Thread>,
        changes: Vec<Change>,
        schedule: Vec<ScheduleChange>,
    ) -> &Entry {
        self.last_id += 1;
        self.undone.clear();
        self.done.push(Entry {
            id: self.last_id,
            action,
            threads,
            changes,
            schedule,
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

    /// The entry a redo would apply next.
    pub fn next_redo(&self) -> Option<&Entry> {
        self.undone.last()
    }

    /// Drops the emails `ids` from the entry with `id`, and each of its
    /// threads left with no changed email, so a redo leaves them alone. An
    /// entry left with nothing is dropped.
    pub fn narrow(&mut self, id: u64, ids: &[String]) {
        let Some(entry) = self
            .done
            .iter_mut()
            .chain(self.undone.iter_mut())
            .find(|e| e.id == id)
        else {
            return;
        };
        entry.changes.retain(|c| !ids.contains(&c.id));
        let changes = &entry.changes;
        entry.threads.retain(|t| {
            t.emails
                .iter()
                .any(|e| changes.iter().any(|c| c.id == e.id))
        });
        if entry.changes.is_empty() {
            self.forget(id);
        }
    }

    /// Drops the entry with `id`, wherever it is.
    pub fn forget(&mut self, id: u64) {
        self.done.retain(|e| e.id != id);
        self.undone.retain(|e| e.id != id);
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }

    /// How many actions can be undone.
    pub fn depth(&self) -> usize {
        self.done.len()
    }
}

/// An undo as one outbox job: `Email/get` on the ids of `expect`,
/// then an `Email/set` of `update` for those still in their expected state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replay {
    /// The state each email must still be in to be changed.
    pub expect: BTreeMap<String, State>,
    pub update: EmailUpdate,
}

impl Replay {
    /// The ids the `Email/get` asks for.
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
