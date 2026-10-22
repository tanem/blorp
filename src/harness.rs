//! A test harness around [`App`]: feeds keys through the keymap into
//! `update` and answers queries and reconciles from a fake store.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::Timestamp;

use crate::app::{App, Effect, Msg, update};
use crate::jmap::fake::Store;
use crate::mail::Mailboxes;
use crate::ui;

/// The time a harness starts at, a Sunday.
pub const NOW: &str = "2026-10-18T12:00:00Z";

pub struct Harness {
    pub app: App,
    store: Store,
}

impl Harness {
    /// An app over `store`'s mailboxes, not yet started.
    pub fn new(store: Store) -> Self {
        let now: Timestamp = NOW.parse().expect("valid timestamp");
        let app = App::new(Mailboxes::from(store.mailboxes()), now);
        Harness { app, store }
    }

    /// An app over `store` with its initial queue loaded.
    pub fn loaded(store: Store) -> Self {
        let mut harness = Harness::new(store);
        let effects = harness.start();
        harness.resolve(effects);
        harness
    }

    pub fn start(&mut self) -> Vec<Effect> {
        self.send(Msg::Start)
    }

    pub fn send(&mut self, msg: Msg) -> Vec<Effect> {
        update(&mut self.app, msg)
    }

    /// Answers every query, reconcile, and `Email/set` in `effects` from
    /// the store, in order, and returns the remaining effects, including
    /// those the answers produce.
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
                    generation, query, ..
                } => {
                    let result = self.store.reconcile(&query);
                    rest.extend(self.send(Msg::Reconciled { generation, result }));
                }
                Effect::SetEmails { job, update } => {
                    let result = Ok(self.store.set_emails(&update));
                    rest.extend(self.send(Msg::Sent { job, result }));
                }
                other => rest.push(other),
            }
        }
        rest
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
