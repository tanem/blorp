//! `f` flags or unflags, and `r` marks read or unread, every email of the
//! selected threads in every mailbox.

use blorp::app::Effect;
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use blorp::jmap::{EmailPatch, EmailUpdate};
use serde_json::Value;

/// Thread t1 spreads over the Inbox, Travel, Sent, Drafts, Trash, and
/// Junk; its Inbox, Trash, and Junk emails are unread, and its Junk email
/// is flagged. Thread t2 is one read, flagged Inbox email, and t3 one read
/// Inbox email. Newest first: t1, t2, t3.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-sent", "Sent", "sent")
        .role_mailbox("mb-drafts", "Drafts", "drafts")
        .role_mailbox("mb-trash", "Trash", "trash")
        .role_mailbox("mb-junk", "Junk", "junk")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1a", "t1", "2026-10-18T08:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Trip"),
        )
        .email(
            EmailSpec::new("e1b", "t1", "2026-10-18T08:10:00Z")
                .in_mailbox("mb-travel")
                .seen(),
        )
        .email(
            EmailSpec::new("e1c", "t1", "2026-10-18T08:20:00Z")
                .in_mailbox("mb-sent")
                .seen(),
        )
        .email(
            EmailSpec::new("e1d", "t1", "2026-10-18T08:30:00Z")
                .in_mailbox("mb-drafts")
                .seen(),
        )
        .email(EmailSpec::new("e1e", "t1", "2026-10-18T08:40:00Z").in_mailbox("mb-trash"))
        .email(
            EmailSpec::new("e1f", "t1", "2026-10-18T08:50:00Z")
                .in_mailbox("mb-junk")
                .flagged(),
        )
        .email(
            EmailSpec::new("e1g", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Re: Trip")
                .seen(),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Lunch")
                .seen()
                .flagged(),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Invoice")
                .seen(),
        )
        .build()
}

/// The patch setting `keyword` on, or off.
fn set(keyword: &str, on: bool) -> EmailPatch {
    let value = if on { Value::Bool(true) } else { Value::Null };
    [(format!("keywords/{keyword}"), value)].into()
}

fn update(ids: &[&str], keyword: &str, on: bool) -> EmailUpdate {
    ids.iter()
        .map(|id| (id.to_string(), set(keyword, on)))
        .collect()
}

/// Whether each row's thread is unread, in order.
fn unread(h: &Harness) -> Vec<bool> {
    h.app.queue().threads().iter().map(|t| t.unread()).collect()
}

/// Whether each row's thread is flagged, in order.
fn flagged(h: &Harness) -> Vec<bool> {
    h.app
        .queue()
        .threads()
        .iter()
        .map(|t| t.flagged())
        .collect()
}

#[test]
fn r_marks_every_email_of_the_thread_read_in_every_mailbox() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e1a", "e1e", "e1f"], "$seen", true),
        }]
    );
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(unread(&h), [false, false, false]);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.status(), "Marked 1 thread read");
}

#[test]
fn r_on_a_thread_with_nothing_unread_marks_every_email_unread() {
    let mut h = Harness::loaded(store());
    h.keys("j");

    let effects = h.keys("r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e2"], "$seen", false),
        }]
    );
    assert_eq!(unread(&h), [true, true, false]);
    assert_eq!(h.status(), "Marked 1 thread unread");
}

#[test]
fn f_on_a_thread_with_a_flagged_email_unflags_every_email() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("f");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e1f"], "$flagged", false),
        }]
    );
    assert_eq!(flagged(&h), [false, true, false]);
    assert_eq!(h.status(), "Unflagged 1 thread");
}

#[test]
fn f_on_a_thread_with_nothing_flagged_flags_every_email() {
    let mut h = Harness::loaded(store());
    h.keys("jj");

    let effects = h.keys("f");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e3"], "$flagged", true),
        }]
    );
    assert_eq!(h.subjects(), ["Re: Trip", "Lunch", "Invoice"]);
    assert_eq!(flagged(&h), [true, true, true]);
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.status(), "Flagged 1 thread");
}

#[test]
fn the_toggle_is_decided_on_the_whole_selection() {
    let mut h = Harness::loaded(store());

    let effects = h.keys("jVjf");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e2"], "$flagged", false),
        }],
        "Lunch is flagged, so both are unflagged and Invoice needs nothing"
    );
    assert_eq!(flagged(&h), [true, false, false]);
    assert_eq!(h.app.queue().visual(), None);
    assert_eq!(h.status(), "Unflagged 1 thread");

    let effects = h.keys("g3r");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 2,
            update: update(&["e1a", "e1e", "e1f"], "$seen", true),
        }],
        "a count from the cursor"
    );
}

#[test]
fn marks_stay_on_threads_a_keyword_action_changed() {
    let mut h = Harness::loaded(store());

    h.keys("xxr");

    assert_eq!(unread(&h), [false, false, false]);
    assert!(h.app.queue().marked(0));
    assert!(h.app.queue().marked(1));
}

#[test]
fn a_read_thread_stays_in_an_unread_queue_and_its_row_updates() {
    let mut h = Harness::loaded(store());
    let effects = h.send(blorp::app::Msg::SetFilter("is:unread".into()));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Trip"]);

    let effects = h.keys("r");
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Trip"]);
    assert_eq!(unread(&h), [false]);
}

#[test]
fn u_undoes_a_keyword_action_and_ctrl_r_redoes_it() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("r");
    h.resolve(effects);
    assert_eq!(h.app.undo_depth(), 1);

    let effects = h.keys("u");
    h.resolve(effects);

    assert_eq!(unread(&h), [true, false, false]);
    assert_eq!(h.status(), "Undone: mark read 1 thread");
    let effects = h.keys("r");
    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 3,
            update: update(&["e1a", "e1e", "e1f"], "$seen", true),
        }],
        "the undo put back exactly the emails that were unread"
    );
    h.resolve(effects);
    let effects = h.keys("u");
    h.resolve(effects);

    let effects = h.ctrl('r');
    h.resolve(effects);

    assert_eq!(unread(&h), [false, false, false]);
    assert_eq!(h.status(), "Redone: mark read 1 thread");
}

#[test]
fn dot_toggles_again_on_the_selection_as_it_is_then() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("r");
    h.resolve(effects);

    let effects = h.keys(".");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 2,
            update: update(
                &["e1a", "e1b", "e1c", "e1d", "e1e", "e1f", "e1g"],
                "$seen",
                false
            ),
        }]
    );
    h.resolve(effects);
    assert_eq!(h.status(), "Marked 1 thread unread");
    assert_eq!(h.app.undo_depth(), 2);

    let effects = h.keys("j.");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 3,
            update: update(&["e2"], "$seen", false),
        }]
    );
}

#[test]
fn a_failed_keyword_action_is_rolled_back() {
    let mut h = Harness::loaded(store());
    h.keys("f");

    h.send(blorp::app::Msg::Sent {
        job: 1,
        result: Err(blorp::jmap::Error::Status(400)),
    });

    assert_eq!(flagged(&h), [true, true, false]);
    assert_eq!(h.app.undo_depth(), 0);
    assert_eq!(
        h.status(),
        "Could not unflag 1 thread: Fastmail error: HTTP 400"
    );
}

#[test]
fn a_dry_run_keyword_action_changes_the_row_and_logs() {
    let mut h = Harness::loaded_dry(store());

    let effects = h.keys("jf");

    assert_eq!(
        effects,
        [Effect::LogDryRun {
            step: blorp::app::Step::Act,
            action: blorp::app::Action::Flag(false),
            thread_ids: vec!["t2".into()],
        }]
    );
    assert_eq!(flagged(&h), [true, false, false]);
    assert_eq!(h.status(), "[dry] Unflagged 1 thread");
}

/// Types `:`, then `text`, then Enter.
fn command(h: &mut Harness, text: &str) -> Vec<Effect> {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(h.send(blorp::app::Msg::Prompt(blorp::prompt::Key::Enter)));
    effects
}

#[test]
fn flag_and_f_toggle_as_f_does() {
    for text in ["flag", "f"] {
        let mut h = Harness::loaded(store());

        h.keys("j");
        let effects = command(&mut h, text);

        assert_eq!(
            effects,
            [Effect::SetEmails {
                job: 1,
                update: update(&["e2"], "$flagged", false),
            }],
            "{text:?}"
        );
        assert_eq!(h.status(), "Unflagged 1 thread", "{text:?}");
    }
}

#[test]
fn read_and_r_mark_read_and_unread_marks_unread_with_the_count_before_the_colon() {
    for text in ["read", "r"] {
        let mut h = Harness::loaded(store());
        h.keys("2");

        let effects = command(&mut h, text);

        assert_eq!(
            effects,
            [Effect::SetEmails {
                job: 1,
                update: update(&["e1a", "e1e", "e1f"], "$seen", true),
            }],
            "{text:?}"
        );
        assert_eq!(h.status(), "Marked 1 thread read", "{text:?}");
    }

    let mut h = Harness::loaded(store());
    h.keys("2");

    let effects = command(&mut h, "unread");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 1,
            update: update(&["e1b", "e1c", "e1d", "e1g", "e2"], "$seen", false),
        }]
    );
    assert_eq!(h.status(), "Marked 2 threads unread");
}

#[test]
fn read_on_a_read_selection_does_nothing_and_says_so() {
    let mut h = Harness::loaded(store());
    h.keys("j");

    let effects = command(&mut h, "read");

    assert_eq!(effects, []);
    assert_eq!(h.status(), "Nothing to mark read");
    assert_eq!(h.app.undo_depth(), 0);
}

#[test]
fn dot_repeats_read_and_unread_as_set() {
    let mut h = Harness::loaded(store());
    let effects = command(&mut h, "unread");
    h.resolve(effects);

    let effects = h.keys("j.");

    assert_eq!(
        effects,
        [Effect::SetEmails {
            job: 2,
            update: update(&["e2"], "$seen", false),
        }]
    );
    h.resolve(effects);

    let effects = h.keys(".");

    assert_eq!(effects, [], "Lunch is already unread");
    assert_eq!(h.status(), "Nothing to mark unread");
}

#[test]
fn keyword_commands_with_nothing_selected_say_so() {
    for text in ["flag", "read", "unread"] {
        let mut h = Harness::loaded(
            Store::builder()
                .role_mailbox("mb-inbox", "Inbox", "inbox")
                .build(),
        );

        assert_eq!(command(&mut h, text), [], "{text:?}");
        assert_eq!(h.status(), "Nothing selected", "{text:?}");
    }
}
