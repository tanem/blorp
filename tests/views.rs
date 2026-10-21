//! Views: the tabline's named filters and sorts, switched with Tab and
//! `:view`, saved to the config file.

use blorp::app::{Effect, Sort};
use blorp::config::View;
use blorp::harness::{CONFIG_FILE, Harness};
use blorp::jmap::fake::{EmailSpec, Store};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Inbox threads "One" (unread, flagged), "Two" (unread), and "Three",
/// newest first, and "Flight" in Travel.
fn store() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .mailbox("mb-travel", "Travel")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-inbox")
                .flagged()
                .subject("One"),
        )
        .email(
            EmailSpec::new("e2", "t2", "2026-10-18T10:00:00Z")
                .in_mailbox("mb-inbox")
                .subject("Two"),
        )
        .email(
            EmailSpec::new("e3", "t3", "2026-10-18T09:00:00Z")
                .in_mailbox("mb-inbox")
                .seen()
                .subject("Three"),
        )
        .email(
            EmailSpec::new("e9", "t9", "2026-10-17T09:00:00Z")
                .in_mailbox("mb-travel")
                .seen()
                .subject("Flight"),
        )
        .build()
}

fn view(name: &str, filter: &str, sort: Sort) -> View {
    View {
        name: name.into(),
        filter: filter.into(),
        sort,
    }
}

fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    let modifiers = if code == KeyCode::BackTab {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    h.key(KeyEvent::new(code, modifiers))
}

/// Presses `code` and answers every request.
fn switch(h: &mut Harness, code: KeyCode) {
    let effects = press(h, code);
    let rest = h.resolve(effects);
    assert_eq!(rest, []);
}

/// Types `:`, then `text`, then Enter, answering every request, and
/// returns what is left.
fn run(h: &mut Harness, text: &str) -> Vec<Effect> {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(press(h, KeyCode::Enter));
    h.resolve(effects)
}

/// The name of each view, in tabline order.
fn names(h: &Harness) -> Vec<String> {
    h.app.views().iter().map(|v| v.name.clone()).collect()
}

/// The name of the view marked active, if one is.
fn active(h: &Harness) -> Option<String> {
    h.app.active_view().map(|i| h.app.views()[i].name.clone())
}

#[test]
fn the_default_views_are_inbox_unread_flagged_and_snoozed_with_inbox_active() {
    let h = Harness::loaded(store());

    assert_eq!(names(&h), ["Inbox", "Unread", "Flagged", "Snoozed"]);
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
}

#[test]
fn tab_and_shift_tab_switch_to_the_next_and_previous_view_wrapping_round() {
    let mut h = Harness::loaded(store());

    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Unread"));
    assert_eq!(h.app.filter(), "is:unread");
    assert_eq!(h.subjects(), ["One", "Two"]);

    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Flagged"));
    assert_eq!(h.subjects(), ["One"]);

    switch(&mut h, KeyCode::BackTab);
    switch(&mut h, KeyCode::BackTab);
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);

    switch(&mut h, KeyCode::BackTab);
    assert_eq!(active(&h).as_deref(), Some("Snoozed"));
    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
}

/// A harness over [`store`] with `views` read from the config file.
fn with_views(views: Vec<View>) -> Harness {
    Harness::loaded_with_views(store(), views)
}

#[test]
fn switching_sets_the_view_sort_and_t_and_sort_change_the_queue_but_not_the_view() {
    let mut h = with_views(vec![
        view("Inbox", "", Sort::Newest),
        view("Oldest", "", Sort::Oldest),
    ]);

    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Oldest"));
    assert_eq!(h.app.sort(), Sort::Oldest);
    assert_eq!(h.subjects(), ["Three", "Two", "One"]);

    let effects = h.keys("t");
    h.resolve(effects);
    assert_eq!(h.app.sort(), Sort::Newest);
    assert_eq!(
        active(&h).as_deref(),
        Some("Oldest"),
        "the filter is unchanged"
    );
    run(&mut h, "sort newest");
    assert_eq!(h.app.views()[1], view("Oldest", "", Sort::Oldest));

    switch(&mut h, KeyCode::BackTab);
    switch(&mut h, KeyCode::Tab);
    assert_eq!(h.app.sort(), Sort::Oldest);
    assert_eq!(h.subjects(), ["Three", "Two", "One"]);
}

#[test]
fn a_filter_no_view_has_marks_none_active_and_tab_goes_to_the_first_view() {
    let mut h = Harness::loaded(store());
    switch(&mut h, KeyCode::Tab);
    run(&mut h, "filter in:travel");

    assert_eq!(active(&h), None);
    assert_eq!(h.app.filter(), "in:travel");

    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
    run(&mut h, "filter in:travel");
    switch(&mut h, KeyCode::BackTab);
    assert_eq!(active(&h).as_deref(), Some("Snoozed"));
}

#[test]
fn a_filter_typed_at_the_prompt_that_matches_a_view_marks_it_active() {
    let mut h = Harness::loaded(store());

    let mut effects = h.keys("/is:flagged");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);

    assert_eq!(active(&h).as_deref(), Some("Flagged"));
}

#[test]
fn a_count_before_tab_is_dropped() {
    let mut h = Harness::loaded(store());
    h.keys("2");

    switch(&mut h, KeyCode::Tab);

    assert_eq!(active(&h).as_deref(), Some("Unread"));
    assert_eq!(h.pending_count(), None);
}

#[test]
fn before_the_snooze_mailbox_exists_the_snoozed_view_is_an_empty_queue() {
    let mut h = Harness::loaded(store());

    switch(&mut h, KeyCode::BackTab);

    assert_eq!(active(&h).as_deref(), Some("Snoozed"));
    assert_eq!(h.app.queue_name(), "Snoozed (blorp)");
    assert_eq!(h.subjects(), Vec::<String>::new());
    assert_eq!(h.status(), "No snoozed threads");
    assert_eq!(h.app.sort_name(), "soonest");

    switch(&mut h, KeyCode::Tab);
    assert_eq!(h.status(), "");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn before_the_snooze_mailbox_exists_any_filter_naming_it_is_an_empty_queue() {
    for text in [
        "filter in:\"snoozed (blorp)\"",
        "filter in:\"Snoozed (blorp)\" is:unread",
    ] {
        let mut h = Harness::loaded(store());

        let rest = run(&mut h, text);

        assert_eq!(rest, [], "{text}");
        assert_eq!(h.subjects(), Vec::<String>::new(), "{text}");
        assert_eq!(h.status(), "No snoozed threads", "{text}");
    }
    let mut h = Harness::loaded(store());
    run(&mut h, "filter -in:\"Snoozed (blorp)\"");
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn once_the_snooze_mailbox_exists_the_snoozed_view_shows_it() {
    let store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .mailbox("mb-snooze", "Snoozed (blorp)")
        .email(
            EmailSpec::new("e1", "t1", "2026-10-18T11:00:00Z")
                .in_mailbox("mb-snooze")
                .subject("Later"),
        )
        .build();
    let mut h = Harness::loaded(store);

    switch(&mut h, KeyCode::BackTab);

    assert_eq!(h.subjects(), ["Later"]);
    assert_eq!(h.status(), "");
}

#[test]
fn view_and_v_switch_to_the_view_named() {
    let mut h = Harness::loaded(store());

    assert_eq!(run(&mut h, "view Flagged"), []);
    assert_eq!(active(&h).as_deref(), Some("Flagged"));
    assert_eq!(h.subjects(), ["One"]);

    assert_eq!(run(&mut h, "v Unread"), []);
    assert_eq!(active(&h).as_deref(), Some("Unread"));
}

#[test]
fn view_errors_leave_the_queue_and_views_as_they_were() {
    for (text, message) in [
        ("view", "Usage: :view [save|delete] <name>"),
        ("view save", "Usage: :view [save|delete] <name>"),
        ("v delete", "Usage: :view [save|delete] <name>"),
        ("view Later", "No view named \"Later\""),
        ("view flagged", "No view named \"flagged\""),
        ("view delete Later", "No view named \"Later\""),
        ("view save save", "View name \"save\" is reserved"),
        ("view save delete", "View name \"delete\" is reserved"),
    ] {
        let mut h = Harness::loaded(store());

        let rest = run(&mut h, text);

        assert_eq!(rest, [], "{text:?}");
        assert_eq!(h.status(), message, "{text:?}");
        assert_eq!(
            names(&h),
            ["Inbox", "Unread", "Flagged", "Snoozed"],
            "{text:?}"
        );
        assert_eq!(h.subjects(), ["One", "Two", "Three"], "{text:?}");
    }
}

#[test]
fn a_view_whose_filter_fails_reports_it_and_leaves_the_queue() {
    let mut h = with_views(vec![
        view("Inbox", "", Sort::Newest),
        view("Gone", "in:Holidays", Sort::Newest),
    ]);

    switch(&mut h, KeyCode::Tab);

    assert_eq!(h.status(), "No mailbox matches \"Holidays\"");
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

#[test]
fn the_first_view_save_writes_the_full_list_with_the_defaults() {
    let mut h = Harness::loaded(store());
    run(&mut h, "filter in:travel");
    let effects = h.keys("t");
    h.resolve(effects);

    let rest = run(&mut h, "view save Travel trips");

    assert_eq!(
        rest,
        [Effect::WriteConfig {
            path: CONFIG_FILE.into(),
            contents: r#"[[views]]
name = "Inbox"
filter = ""

[[views]]
name = "Unread"
filter = "is:unread"

[[views]]
name = "Flagged"
filter = "is:flagged"

[[views]]
name = "Snoozed"
filter = 'in:"Snoozed (blorp)"'

[[views]]
name = "Travel trips"
filter = "in:travel"
sort = "oldest"
"#
            .into(),
        }]
    );
    assert_eq!(h.status(), "Saved view \"Travel trips\"");
    assert_eq!(
        names(&h),
        ["Inbox", "Unread", "Flagged", "Snoozed", "Travel trips"]
    );
    assert_eq!(active(&h).as_deref(), Some("Travel trips"));
}

#[test]
fn view_save_overwrites_a_view_of_the_same_name_in_place() {
    let mut h = Harness::loaded(store());
    run(&mut h, "filter is:unread -is:flagged");

    let rest = run(&mut h, "view save Unread");

    assert_eq!(rest.len(), 1, "{rest:?}");
    assert_eq!(names(&h), ["Inbox", "Unread", "Flagged", "Snoozed"]);
    assert_eq!(
        h.app.views()[1],
        view("Unread", "is:unread -is:flagged", Sort::Newest)
    );
    assert_eq!(active(&h).as_deref(), Some("Unread"));
}

#[test]
fn a_saved_view_with_the_filter_of_another_is_the_one_marked_active() {
    let mut h = Harness::loaded(store());
    let effects = h.keys("t");
    h.resolve(effects);

    run(&mut h, "view save Oldest");

    assert_eq!(active(&h).as_deref(), Some("Oldest"));
    switch(&mut h, KeyCode::Tab);
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
    assert_eq!(h.app.sort(), Sort::Newest);
}

#[test]
fn view_delete_removes_the_view_and_writes_the_rest() {
    let mut h = Harness::loaded(store());
    switch(&mut h, KeyCode::Tab);

    let rest = run(&mut h, "view delete Unread");

    assert_eq!(names(&h), ["Inbox", "Flagged", "Snoozed"]);
    assert_eq!(h.status(), "Deleted view \"Unread\"");
    let [Effect::WriteConfig { path, contents }] = &rest[..] else {
        panic!("{rest:?}");
    };
    assert_eq!(path.to_str(), Some(CONFIG_FILE));
    assert!(!contents.contains("Unread"), "{contents}");
    assert_eq!(
        active(&h),
        None,
        "the queue keeps the deleted view's filter"
    );
    assert_eq!(h.app.filter(), "is:unread");
}

#[test]
fn deleting_every_view_writes_an_empty_list_and_tab_does_nothing() {
    let mut h = with_views(vec![view("Inbox", "", Sort::Newest)]);

    let rest = run(&mut h, "view delete Inbox");

    assert_eq!(
        rest,
        [Effect::WriteConfig {
            path: CONFIG_FILE.into(),
            contents: "views = []\n".into(),
        }]
    );
    assert_eq!(press(&mut h, KeyCode::Tab), []);
    assert_eq!(press(&mut h, KeyCode::BackTab), []);
}

#[test]
fn a_config_write_that_fails_is_reported_and_the_change_stays_for_the_session() {
    let mut h = Harness::loaded(store());
    run(&mut h, "view delete Unread");

    h.send(blorp::app::Msg::ConfigFailed {
        error: "Permission denied (os error 13)".into(),
    });

    assert_eq!(
        h.status(),
        "Could not save the views: Permission denied (os error 13)"
    );
    assert_eq!(names(&h), ["Inbox", "Flagged", "Snoozed"]);
}

#[test]
fn without_a_config_file_a_save_lasts_the_session() {
    let mut app = blorp::app::App::new(
        blorp::mail::Mailboxes::default(),
        "2026-10-18T12:00:00Z".parse().unwrap(),
    );

    let effects = blorp::app::update(&mut app, blorp::app::Msg::SaveView("Mine".into()));

    assert_eq!(effects, []);
    assert_eq!(app.views().last().unwrap().name, "Mine");
    assert_eq!(
        app.status(),
        "Saved view \"Mine\" for this session: no config file"
    );
}

#[test]
fn view_save_and_delete_go_ahead_while_disconnected() {
    let mut h = Harness::loaded(store());
    let generation = match h.send(blorp::app::Msg::Refresh)[..] {
        [Effect::Reconcile { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    h.send(blorp::app::Msg::Reconciled {
        generation,
        result: Err(blorp::jmap::Error::Transport("connection refused".into())),
    });
    assert!(!h.app.connected());

    let rest = run(&mut h, "view save Mine");

    assert!(matches!(rest[..], [Effect::WriteConfig { .. }]), "{rest:?}");
    assert_eq!(h.status(), "Saved view \"Mine\"");
}

#[test]
fn a_view_switch_that_fails_while_disconnected_leaves_the_queue_and_filter() {
    let mut h = Harness::loaded(store());
    let generation = match h.send(blorp::app::Msg::Refresh)[..] {
        [Effect::Reconcile { generation, .. }] => generation,
        ref other => panic!("{other:?}"),
    };
    h.send(blorp::app::Msg::Reconciled {
        generation,
        result: Err(blorp::jmap::Error::Transport("connection refused".into())),
    });

    let effects = press(&mut h, KeyCode::Tab);
    let [
        Effect::Query {
            generation,
            retry: false,
            ..
        },
    ] = effects[..]
    else {
        panic!("{effects:?}");
    };
    h.send(blorp::app::Msg::Loaded {
        generation,
        result: Err(blorp::jmap::Error::Transport("connection refused".into())),
    });

    assert_eq!(h.app.filter(), "");
    assert_eq!(active(&h).as_deref(), Some("Inbox"));
    assert_eq!(h.subjects(), ["One", "Two", "Three"]);
}

/// Opens the command line, types `text`, presses Tab, and returns the
/// text and the candidates.
fn tab(h: &mut Harness, text: &str) -> (String, Vec<String>) {
    h.keys(":");
    h.keys(text);
    press(h, KeyCode::Tab);
    let shown = h
        .app
        .completion()
        .map_or(Vec::new(), |(all, _)| all.to_vec());
    let text = h.app.prompt().unwrap().to_string();
    press(h, KeyCode::Esc);
    (text, shown)
}

#[test]
fn tab_after_view_completes_save_delete_and_the_view_names() {
    let mut h = with_views(vec![
        view("Inbox", "", Sort::Newest),
        view("Work stuff", "in:Work", Sort::Newest),
        view("Weekly", "is:list", Sort::Newest),
        view("Saved", "is:flagged", Sort::Newest),
    ]);

    let (text, shown) = tab(&mut h, "view ");
    assert_eq!(text, "view save");
    assert_eq!(
        shown,
        ["save", "delete", "Inbox", "Work stuff", "Weekly", "Saved"]
    );

    assert_eq!(
        tab(&mut h, "v s"),
        ("v save".into(), vec!["save".into(), "Saved".into()])
    );
    assert_eq!(
        tab(&mut h, "view w"),
        (
            "view Work stuff".into(),
            vec!["Work stuff".into(), "Weekly".into()]
        )
    );
    assert_eq!(
        tab(&mut h, "view Work s"),
        ("view Work stuff".into(), vec!["Work stuff".into()])
    );
}

#[test]
fn tab_after_view_save_or_delete_completes_the_view_names() {
    let mut h = with_views(vec![
        view("Inbox", "", Sort::Newest),
        view("Work stuff", "in:Work", Sort::Newest),
        view("Saved", "is:flagged", Sort::Newest),
    ]);

    assert_eq!(
        tab(&mut h, "view delete "),
        (
            "view delete Inbox".into(),
            vec!["Inbox".into(), "Work stuff".into(), "Saved".into()]
        )
    );
    assert_eq!(
        tab(&mut h, "view save wo"),
        ("view save Work stuff".into(), vec!["Work stuff".into()])
    );
    assert_eq!(
        tab(&mut h, "view delete Work"),
        ("view delete Work stuff".into(), vec!["Work stuff".into()])
    );
    assert_eq!(
        tab(&mut h, "view delete x"),
        ("view delete x".into(), vec![])
    );
}

#[test]
fn the_snoozed_view_opened_while_the_snooze_mailbox_is_being_created_queries_it_once_made() {
    let mut h = Harness::loaded(store());
    let mut effects = h.keys("z");
    effects.extend(h.keys("t"));
    let (create, rest): (Vec<_>, Vec<_>) = effects
        .into_iter()
        .partition(|e| matches!(e, Effect::CreateMailbox { .. }));
    assert_eq!(create.len(), 1, "{rest:?}");

    let effects = press(&mut h, KeyCode::BackTab);
    h.resolve(effects);
    assert_eq!(h.status(), "No snoozed threads");
    h.resolve(create);
    assert_eq!(
        h.status(),
        "Nothing to snooze",
        "its threads left the queue"
    );
    let effects = h.keys("R");

    let [Effect::Reconcile { query, .. }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert!(
        query.filter.get("inMailbox").is_some(),
        "{:?}",
        query.filter
    );
    assert_eq!(h.app.queue_name(), "Snoozed (blorp)");
    assert_eq!(active(&h).as_deref(), Some("Snoozed"));
}
