//! A test harness around [`App`]: feeds keys through the keymap into
//! `update` and answers queries, reconciles, and mailbox creation from a
//! fake store. Schedule writes are left for the test to see.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::Timestamp;

use crate::app::{App, Effect, Msg, update};
use crate::jmap::fake::Store;
use crate::jmap::{EmailQuery, Error, Reconciled};
use crate::mail::Mailboxes;
use crate::snooze::Schedule;
use crate::ui;
use crate::undo::Replayed;

/// Whether [`Harness::resolve`] answers `effect` from the store.
fn answerable(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Query { .. }
            | Effect::Reconcile { .. }
            | Effect::SetEmails { .. }
            | Effect::Replay { .. }
            | Effect::CreateMailbox { .. }
            | Effect::QuerySnoozed { .. }
    )
}

/// The time a harness starts at, a Sunday. The harness's time zone is UTC.
pub const NOW: &str = "2026-10-18T12:00:00Z";

/// Where the harness's app writes the snooze schedule. Nothing is written
/// there: the harness leaves writes as effects.
pub const SCHEDULE_FILE: &str = "/harness/state/blorp/snooze.json";

pub struct Harness {
    pub app: App,
    store: Store,
}

impl Harness {
    /// An app over `store`'s mailboxes, not yet started.
    pub fn new(store: Store) -> Self {
        Harness::build(store, false, Schedule::default())
    }

    fn build(store: Store, dry_run: bool, schedule: Schedule) -> Self {
        let now: Timestamp = NOW.parse().expect("valid timestamp");
        let app = App::new(Mailboxes::from(store.mailboxes()), now)
            .with_schedule(SCHEDULE_FILE.into(), schedule)
            .with_dry_run(dry_run);
        Harness { app, store }
    }

    /// An app over `store` and `schedule`, read from its file, not yet
    /// started.
    pub fn with_schedule(store: Store, schedule: Schedule) -> Self {
        Harness::build(store, false, schedule)
    }

    /// An app over `store` with its initial queue loaded.
    pub fn loaded(store: Store) -> Self {
        let mut harness = Harness::new(store);
        let effects = harness.start();
        harness.resolve(effects);
        harness
    }

    /// An app over `store` started with `--dry-run`, its queue loaded.
    pub fn loaded_dry(store: Store) -> Self {
        Harness::launched(store, true, Schedule::default()).0
    }

    /// An app over `store` started with `schedule` read from its file, its
    /// queue loaded.
    pub fn loaded_with(store: Store, schedule: Schedule) -> Self {
        Harness::launched(store, false, schedule).0
    }

    /// An app over `store` started with `schedule`, in dry run or not, its
    /// queue loaded and any due threads returned, and the launch's effects
    /// left unresolved.
    pub fn launched(store: Store, dry_run: bool, schedule: Schedule) -> (Self, Vec<Effect>) {
        let mut harness = Harness::build(store, dry_run, schedule);
        let effects = harness.start();
        let effects = harness.settle(effects);
        (harness, effects)
    }

    /// Resolves `effects`, then the effects the answers produce, until
    /// none is left that the store answers, and returns the rest in the
    /// order they came.
    pub fn settle(&mut self, effects: Vec<Effect>) -> Vec<Effect> {
        let mut rest = Vec::new();
        let mut pending = effects;
        while !pending.is_empty() {
            let (answered, unanswered): (Vec<_>, Vec<_>) =
                pending.into_iter().partition(answerable);
            rest.extend(unanswered);
            pending = self.resolve(answered);
        }
        rest
    }

    pub fn start(&mut self) -> Vec<Effect> {
        self.send(Msg::Start)
    }

    pub fn send(&mut self, msg: Msg) -> Vec<Effect> {
        update(&mut self.app, msg)
    }

    /// Answers every query, `Email/set`, and undo in `effects` from the
    /// store, in order, and returns the remaining effects, including those
    /// the answers produce.
    pub fn resolve(&mut self, effects: Vec<Effect>) -> Vec<Effect> {
        let mut rest = Vec::new();
        for effect in effects {
            match effect {
                Effect::Query {
                    generation, query, ..
                } => {
                    let result = self.store.query(&query);
                    rest.extend(self.send(Msg::Loaded { generation, result }));
                }
                Effect::Reconcile {
                    generation,
                    query,
                    snoozed,
                    ..
                } => {
                    let result = self.store.reconcile(&query, snoozed.as_ref());
                    rest.extend(self.send(Msg::Reconciled { generation, result }));
                }
                Effect::SetEmails { job, update } => {
                    let result = Ok(self.store.set_emails(&update));
                    rest.extend(self.send(Msg::Sent { job, result }));
                }
                Effect::CreateMailbox { job, name } => {
                    let result = self.store.create_mailbox(&name);
                    rest.extend(self.send(Msg::MailboxCreated { job, result }));
                }
                Effect::QuerySnoozed { query, .. } => {
                    let result = self.store.query(&query);
                    rest.extend(self.send(Msg::SnoozedLoaded { result }));
                }
                Effect::Replay { job, replay } => {
                    let result = self.store.get_emails(&replay.ids()).map(|(got, _)| {
                        let (update, skipped) = replay.split(&got);
                        let not_updated = if update.is_empty() {
                            Vec::new()
                        } else {
                            self.store.set_emails(&update).not_updated
                        };
                        Replayed {
                            skipped,
                            not_updated,
                        }
                    });
                    rest.extend(self.send(Msg::Replayed { job, result }));
                }
                other => rest.push(other),
            }
        }
        rest
    }

    /// What a query of `query` gets from the store now.
    pub fn query(&self, query: &EmailQuery) -> Result<crate::jmap::QueryResult, Error> {
        self.store.query(query)
    }

    /// What a reconcile of `query`, with no snooze mailbox query, gets
    /// from the store now.
    pub fn reconcile(&self, query: &EmailQuery) -> Result<Reconciled, Error> {
        self.store.reconcile(query, None)
    }

    /// Types each character as a key press.
    pub fn keys(&mut self, keys: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        for c in keys.chars() {
            let modifiers = if c.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            effects.extend(self.key(KeyEvent::new(KeyCode::Char(c), modifiers)));
        }
        effects
    }

    /// Presses Ctrl and `c`.
    pub fn ctrl(&mut self, c: char) -> Vec<Effect> {
        self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    pub fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match ui::key_msg(&self.app, key) {
            Some(msg) => self.send(msg),
            None => Vec::new(),
        }
    }

    pub fn cursor(&self) -> usize {
        self.app.queue().cursor()
    }

    pub fn pending_count(&self) -> Option<usize> {
        self.app.pending_count()
    }

    /// Changes the store behind blorp's back, as the web client would.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    pub fn status(&self) -> &str {
        self.app.status()
    }

    /// The subject of each row, in order.
    pub fn subjects(&self) -> Vec<String> {
        self.app
            .queue()
            .threads()
            .iter()
            .map(|t| t.hit().subject.clone().unwrap_or_default())
            .collect()
    }
}
