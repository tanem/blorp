//! `:` opens the command line, which runs actions and settings by name.

use blorp::app::{Effect, Mode, Sort};
use blorp::harness::Harness;
use blorp::jmap::fake::{EmailSpec, Store};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Four single-email Inbox threads, newest first: "One" to "Four". Travel
/// has "Flight". Work has Receipts and Reports, Home has Receipts, and Old
/// Stuff has a space in its name. Drafts and Sent cannot be a queue.
fn store() -> Store {
    let mut store = Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .role_mailbox("mb-drafts", "Drafts", "drafts")
        .role_mailbox("mb-sent", "Sent", "sent")
        .mailbox("mb-travel", "Travel")
        .mailbox("mb-work", "Work")
        .child_mailbox("mb-work-receipts", "Receipts", "mb-work")
        .child_mailbox("mb-work-reports", "Reports", "mb-work")
        .mailbox("mb-home", "Home")
        .child_mailbox("mb-home-receipts", "Receipts", "mb-home")
        .mailbox("mb-old", "Old Stuff")
        .email(
            EmailSpec::new("e9", "t9", "2026-10-17T09:00:00Z")
                .in_mailbox("mb-travel")
                .subject("Flight"),
        );
    for (i, subject) in ["One", "Two", "Three", "Four"].iter().enumerate() {
        let n = i + 1;
        let received = format!("2026-10-18T1{}:00:00Z", 5 - n);
        store = store.email(
            EmailSpec::new(&format!("e{n}"), &format!("t{n}"), &received)
                .in_mailbox("mb-inbox")
                .subject(subject),
        );
    }
    store.build()
}

fn press(h: &mut Harness, code: KeyCode) -> Vec<Effect> {
    h.key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Types `:`, then `text`, then Enter, answering every request.
fn run(h: &mut Harness, text: &str) {
    let mut effects = h.keys(":");
    effects.extend(h.keys(text));
    effects.extend(press(h, KeyCode::Enter));
    let rest = h.resolve(effects);
    assert_eq!(rest, [], "{text:?}");
}

#[test]
fn colon_opens_the_command_line_and_enter_runs_the_command() {
    let mut h = Harness::loaded(store());

    h.keys(":archive");
    assert_eq!(h.app.mode(), Mode::Command);
    assert_eq!(h.app.prompt(), Some("archive"));
    let effects = press(&mut h, KeyCode::Enter);
    h.resolve(effects);

    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.subjects(), ["Two", "Three", "Four"]);
    assert_eq!(h.status(), "Archived 1 thread");
}

#[test]
fn a_command_acts_on_the_count_typed_before_the_colon() {
    let mut h = Harness::loaded(store());
    h.keys("j");

    let mut effects = h.keys("2:");
    assert_eq!(h.pending_count(), None);
    effects.extend(h.keys("e"));
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);

    assert_eq!(h.subjects(), ["One", "Four"]);
    assert_eq!(h.status(), "Archived 2 threads");
    assert_eq!(h.pending_count(), None);
}

#[test]
fn a_command_acts_on_the_visual_range_open_when_colon_is_pressed() {
    let mut h = Harness::loaded(store());
    h.keys("Vj");

    h.keys(":");
    assert_eq!(h.app.mode(), Mode::Command);
    assert_eq!(h.app.queue().visual(), Some(0..=1));
    assert_eq!(h.app.prompt(), Some(""));
    let mut effects = h.keys("archive");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);

    assert_eq!(h.subjects(), ["Three", "Four"]);
    assert_eq!(h.app.mode(), Mode::Normal);
}

#[test]
fn a_command_acts_on_the_marked_rows() {
    let mut h = Harness::loaded(store());
    h.keys("xjjx");

    run(&mut h, "e");

    assert_eq!(h.subjects(), ["Two", "Three"]);
}

#[test]
fn a_command_that_changes_nothing_still_returns_to_normal_mode() {
    let mut h = Harness::loaded(store());
    h.keys("V");

    run(&mut h, "sort newest");

    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.app.queue().visual(), None);
}

#[test]
fn esc_closes_the_command_line_and_runs_nothing() {
    let mut h = Harness::loaded(store());
    h.keys("V3:archive");

    let effects = press(&mut h, KeyCode::Esc);

    assert_eq!(effects, []);
    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.app.prompt(), None);
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"]);
    assert_eq!(h.pending_count(), None);
}

#[test]
fn archive_with_nothing_selected_says_so() {
    let mut h = Harness::loaded(store());
    run(&mut h, "filter in:archive");

    run(&mut h, "archive");

    assert_eq!(h.status(), "Nothing selected");
}

#[test]
fn undo_takes_a_count_and_redo_takes_the_one_before_the_colon() {
    let mut h = Harness::loaded(store());
    run(&mut h, "e");
    run(&mut h, "e");
    run(&mut h, "e");
    assert_eq!(h.subjects(), ["Four"]);

    run(&mut h, "undo 2");
    assert_eq!(h.subjects(), ["Two", "Three", "Four"]);
    assert_eq!(h.app.undo_depth(), 1);

    run(&mut h, "u");
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"]);

    let mut effects = h.keys("2:redo");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Three", "Four"]);
    assert_eq!(h.app.undo_depth(), 2);
}

#[test]
fn filter_selects_the_queue_and_with_no_query_goes_back_to_the_inbox() {
    let mut h = Harness::loaded(store());

    run(&mut h, "filter in:travel");
    assert_eq!(h.subjects(), ["Flight"]);
    assert_eq!(h.app.filter(), "in:travel");

    run(&mut h, "filter");
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"]);
    assert_eq!(h.app.filter(), "");
}

#[test]
fn a_filter_error_leaves_the_queue_as_it_is() {
    let mut h = Harness::loaded(store());

    run(&mut h, "filter in:nowhere");

    assert_eq!(h.status(), "No mailbox matches \"nowhere\"");
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"]);
}

#[test]
fn sort_toggles_or_sets_the_order() {
    let mut h = Harness::loaded(store());

    run(&mut h, "sort");
    assert_eq!(h.app.sort(), Sort::Oldest);
    assert_eq!(h.subjects(), ["Four", "Three", "Two", "One"]);

    run(&mut h, "sort oldest");
    assert_eq!(h.app.sort(), Sort::Oldest);

    run(&mut h, "sort newest");
    assert_eq!(h.app.sort(), Sort::Newest);
    assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"]);
}

#[test]
fn dry_turns_dry_run_on_and_dry_off_turns_it_off() {
    let mut h = Harness::loaded(store());
    run(&mut h, "e");

    run(&mut h, "dry");
    assert!(h.app.dry_run());
    assert_eq!(h.status(), "Dry run on, undo history cleared");
    assert_eq!(h.app.undo_depth(), 0);

    let mut effects = h.keys(":e");
    effects.extend(press(&mut h, KeyCode::Enter));
    assert!(matches!(h.resolve(effects)[..], [Effect::LogDryRun { .. }]));
    run(&mut h, "dry");
    assert!(h.app.dry_run());
    assert_eq!(h.status(), "Dry run already on");
    assert_eq!(h.app.undo_depth(), 1);

    run(&mut h, "dry off");
    assert!(!h.app.dry_run());
    assert_eq!(h.status(), "Dry run off, undo history cleared");
}

#[test]
fn quit_and_q_quit() {
    for text in ["quit", "q"] {
        let mut h = Harness::loaded(store());
        let mut effects = h.keys(":");
        effects.extend(h.keys(text));
        effects.extend(press(&mut h, KeyCode::Enter));

        assert_eq!(effects, [Effect::Quit], "{text:?}");
    }
}

#[test]
fn errors_close_the_prompt_and_show_in_the_status_line() {
    for (text, message) in [
        ("foo", "Unknown command: foo"),
        ("set", "Unknown command: set"),
        ("Archive", "Unknown command: Archive"),
        ("arch", "Unknown command: arch"),
        ("archive now", "Usage: :archive"),
        ("redo 2", "Usage: :redo"),
        ("quit now", "Usage: :quit"),
        ("undo two", "Usage: :undo [count]"),
        ("undo 0", "Usage: :undo [count]"),
        ("undo 1 2", "Usage: :undo [count]"),
        ("sort sideways", "Usage: :sort [newest|oldest]"),
        ("dry on", "Usage: :dry [off]"),
    ] {
        let mut h = Harness::loaded(store());

        run(&mut h, text);

        assert_eq!(h.status(), message, "{text:?}");
        assert_eq!(h.app.prompt(), None, "{text:?}");
        assert_eq!(h.subjects(), ["One", "Two", "Three", "Four"], "{text:?}");
    }
}

#[test]
fn colon_clears_the_status_message_and_an_empty_command_does_nothing() {
    let mut h = Harness::loaded(store());
    run(&mut h, "foo");

    h.keys(":");
    assert_eq!(h.status(), "");
    let effects = press(&mut h, KeyCode::Enter);

    assert_eq!(effects, []);
    assert_eq!(h.app.mode(), Mode::Normal);
    assert_eq!(h.status(), "");
}

#[test]
fn up_and_down_walk_the_commands_run_including_failed_ones() {
    let mut h = Harness::loaded(store());
    run(&mut h, "sort newest");
    run(&mut h, "filter in:nowhere");
    run(&mut h, "sort oldest");

    h.keys(":");
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("sort oldest"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("filter in:nowhere"));
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.prompt(), Some("sort oldest"));
    press(&mut h, KeyCode::Down);
    assert_eq!(h.app.prompt(), Some(""));
}

#[test]
fn up_walks_only_the_commands_that_start_with_the_typed_text() {
    let mut h = Harness::loaded(store());
    run(&mut h, "sort newest");
    run(&mut h, "filter in:travel");
    run(&mut h, "sort oldest");

    h.keys(":so");
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("sort oldest"));
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("sort newest"));
}

#[test]
fn the_command_line_and_the_filter_prompt_keep_separate_histories() {
    let mut h = Harness::loaded(store());
    run(&mut h, "sort oldest");
    let mut effects = h.keys("/in:travel");
    effects.extend(press(&mut h, KeyCode::Enter));
    h.resolve(effects);

    h.keys(":");
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("sort oldest"));
    press(&mut h, KeyCode::Esc);

    h.keys("/");
    h.ctrl('u');
    press(&mut h, KeyCode::Up);
    assert_eq!(h.app.prompt(), Some("in:travel"));
}

#[test]
fn the_editing_keys_edit_the_command_line() {
    let mut h = Harness::loaded(store());

    h.keys(":sort oldest");
    h.ctrl('w');
    assert_eq!(h.app.prompt(), Some("sort "));
    h.ctrl('a');
    h.keys("x");
    assert_eq!(h.app.prompt(), Some("xsort "));
    press(&mut h, KeyCode::Right);
    press(&mut h, KeyCode::Backspace);
    assert_eq!(h.app.prompt(), Some("xort "));
    h.ctrl('e');
    press(&mut h, KeyCode::Left);
    assert_eq!(h.app.prompt_cursor(), Some(4));
    press(&mut h, KeyCode::Home);
    assert_eq!(h.app.prompt_cursor(), Some(0));
    press(&mut h, KeyCode::End);
    assert_eq!(h.app.prompt_cursor(), Some(5));
    h.ctrl('u');
    assert_eq!(h.app.prompt(), Some(""));
    assert_eq!(h.app.mode(), Mode::Command);
}

#[test]
fn ctrl_c_quits_from_the_command_line() {
    let mut h = Harness::loaded(store());
    h.keys(":arch");

    assert_eq!(h.ctrl('c'), [Effect::Quit]);
}

/// The candidates on the line above the prompt, and the one chosen.
fn candidates(h: &Harness) -> Option<(Vec<&str>, usize)> {
    h.app
        .completion()
        .map(|(all, index)| (all.iter().map(String::as_str).collect(), index))
}

/// Opens the command line, types `text`, presses Tab, and returns the
/// text and the candidates.
fn tab(text: &str) -> (String, Vec<String>) {
    let mut h = Harness::loaded(store());
    h.keys(":");
    h.keys(text);
    press(&mut h, KeyCode::Tab);
    let shown = h
        .app
        .completion()
        .map_or(Vec::new(), |(all, _)| all.to_vec());
    (h.app.prompt().unwrap().to_string(), shown)
}

#[test]
fn tab_and_shift_tab_cycle_through_the_command_names_and_aliases() {
    let mut h = Harness::loaded(store());
    h.keys(":");

    press(&mut h, KeyCode::Tab);
    let names = vec![
        "archive", "e", "snooze", "z", "undo", "u", "redo", "filter", "sort", "refresh", "dry",
        "quit", "q",
    ];
    assert_eq!(candidates(&h), Some((names.clone(), 0)));
    assert_eq!(h.app.prompt(), Some("archive"));
    press(&mut h, KeyCode::Tab);
    assert_eq!(h.app.prompt(), Some("e"));
    assert_eq!(candidates(&h), Some((names.clone(), 1)));
    press(&mut h, KeyCode::BackTab);
    press(&mut h, KeyCode::BackTab);
    assert_eq!(h.app.prompt(), Some("q"));
    assert_eq!(candidates(&h), Some((names, 12)));
    assert_eq!(h.app.prompt_cursor(), Some(1));
}

#[test]
fn shift_tab_starts_from_the_last_candidate() {
    let mut h = Harness::loaded(store());
    h.keys(":u");

    press(&mut h, KeyCode::BackTab);

    assert_eq!(h.app.prompt(), Some("u"));
    assert_eq!(candidates(&h), Some((vec!["undo", "u"], 1)));
}

#[test]
fn typing_after_a_completion_keeps_it_and_closes_the_candidate_line() {
    let mut h = Harness::loaded(store());
    h.keys(":so");
    press(&mut h, KeyCode::Tab);
    assert_eq!(h.app.prompt(), Some("sort"));

    h.keys(" ");

    assert_eq!(h.app.prompt(), Some("sort "));
    assert_eq!(candidates(&h), None);
    press(&mut h, KeyCode::Tab);
    assert_eq!(h.app.prompt(), Some("sort newest"));
    assert_eq!(candidates(&h), Some((vec!["newest", "oldest"], 0)));
}

#[test]
fn tab_completes_the_word_under_the_cursor() {
    for (text, completed, shown) in [
        ("sort o", "sort oldest", vec!["oldest"]),
        ("sort newest o", "sort newest o", vec![]),
        ("filter in:d", "filter in:d", vec![]),
        ("filter in:s", "filter in:s", vec![]),
        ("filter -in:d", "filter -in:Drafts", vec!["-in:Drafts"]),
        ("arc", "archive", vec!["archive"]),
        ("xyz", "xyz", vec![]),
        ("Ar", "Ar", vec![]),
        ("archive ", "archive ", vec![]),
        ("dry ", "dry ", vec![]),
        (
            "filter ",
            "filter in:",
            vec!["in:", "from:", "to:", "subject:", "is:", "has:"],
        ),
        ("filter i", "filter in:", vec!["in:", "is:"]),
        ("filter s", "filter subject:", vec!["subject:"]),
        ("filter -f", "filter -from:", vec!["-from:"]),
        ("filter lunch", "filter lunch", vec![]),
        (
            "filter is:",
            "filter is:unread",
            vec!["is:unread", "is:read", "is:flagged", "is:list"],
        ),
        (
            "filter from:ana is:f",
            "filter from:ana is:flagged",
            vec!["is:flagged"],
        ),
        ("filter -is:r", "filter -is:read", vec!["-is:read"]),
        (
            "filter has:",
            "filter has:attachment",
            vec!["has:attachment"],
        ),
        ("filter to:a", "filter to:a", vec![]),
        ("filter in:x", "filter in:x", vec![]),
        (
            "filter in:",
            "filter in:Archive",
            vec![
                "in:Archive",
                "in:Home",
                "in:Inbox",
                "in:\"Old Stuff\"",
                "in:Travel",
                "in:Work",
            ],
        ),
        ("filter in:tr", "filter in:Travel", vec!["in:Travel"]),
        (
            "filter in:Work/Re",
            "filter in:Work/Receipts",
            vec!["in:Work/Receipts", "in:Work/Reports"],
        ),
        (
            "filter in:home/r",
            "filter in:Home/Receipts",
            vec!["in:Home/Receipts"],
        ),
        (
            "filter -in:work/rep",
            "filter -in:Work/Reports",
            vec!["-in:Work/Reports"],
        ),
        (
            "filter in:ol",
            "filter in:\"Old Stuff\"",
            vec!["in:\"Old Stuff\""],
        ),
        (
            "filter in:\"o",
            "filter in:\"Old Stuff\"",
            vec!["in:\"Old Stuff\""],
        ),
        (
            "filter in:\"work/rec",
            "filter in:\"Work/Receipts\"",
            vec!["in:\"Work/Receipts\""],
        ),
        (
            "filter in:\"Old S",
            "filter in:\"Old Stuff\"",
            vec!["in:\"Old Stuff\""],
        ),
    ] {
        let (text_after, candidates) = tab(text);
        assert_eq!(text_after, completed, "{text:?}");
        assert_eq!(candidates, shown, "{text:?}");
    }
}

#[test]
fn tab_replaces_the_whole_word_and_leaves_the_rest_of_the_line() {
    let mut h = Harness::loaded(store());
    h.keys(":filter is:unrxx from:ana");
    for _ in 0.." from:ana".len() + 2 {
        press(&mut h, KeyCode::Left);
    }

    press(&mut h, KeyCode::Tab);

    assert_eq!(h.app.prompt(), Some("filter is:unread from:ana"));
    assert_eq!(h.app.prompt_cursor(), Some("filter is:unread".len()));
}

#[test]
fn enter_runs_the_completed_command_and_esc_closes_the_candidate_line() {
    let mut h = Harness::loaded(store());
    h.keys(":filter in:tr");
    press(&mut h, KeyCode::Tab);
    let effects = press(&mut h, KeyCode::Enter);
    h.resolve(effects);
    assert_eq!(h.subjects(), ["Flight"]);
    assert_eq!(candidates(&h), None);

    h.keys(":s");
    press(&mut h, KeyCode::Tab);
    press(&mut h, KeyCode::Esc);
    assert_eq!(candidates(&h), None);
    assert_eq!(h.app.mode(), Mode::Normal);
}
