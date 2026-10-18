//! The blorp binary in a pseudo-terminal against a local JMAP server: the
//! Inbox, motions and counts, resize, quitting, and a rejected token.

use blorp::jmap::fake::{EmailSpec, Store};
use jiff::{SignedDuration, Timestamp};

mod e2e;

use e2e::server::LocalJmap;
use e2e::{Blorp, Modes, Sandbox, statusline};

const TOKEN: &str = "fake-token-1";

/// RFC 3339 for `minutes` before now, so ages on screen do not depend on
/// the date the tests run.
fn ago(minutes: i64) -> String {
    (Timestamp::now() - SignedDuration::from_mins(minutes)).to_string()
}

/// The `ui` snapshot fixture, with its ages taken from now.
fn busy_inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(
            EmailSpec::new("e1", "t1", &ago(2))
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject("Lunch on Friday?"),
        )
        .email(
            EmailSpec::new("e2a", "t2", &ago(4 * 24 * 60))
                .in_mailbox("mb-archive")
                .seen()
                .from("Kim Park", "kim@example.invalid")
                .subject("Trip to Wellington"),
        )
        .email(
            EmailSpec::new("e2b", "t2", &ago(5 * 60 + 20))
                .in_mailbox("mb-inbox")
                .seen()
                .from("Ravi Shah", "ravi@example.invalid")
                .subject("Re: Trip to Wellington"),
        )
        .email(
            EmailSpec::new("e2c", "t2", &ago(5 * 60 + 10))
                .in_mailbox("mb-archive")
                .seen()
                .from("Kim Park", "kim@example.invalid")
                .subject("Re: Trip to Wellington"),
        )
        .email(
            EmailSpec::new("e3", "t3", &ago(3 * 24 * 60 + 60))
                .in_mailbox("mb-inbox")
                .seen()
                .flagged()
                .from("", "billing@shop.example.invalid")
                .subject(
                    "Your order 48213 has shipped and will arrive between Tuesday and Thursday next week",
                ),
        )
        .email(
            EmailSpec::new("e4", "t4", &ago(30 * 24 * 60))
                .in_mailbox("mb-inbox")
                .flagged()
                .from("Maximiliana Featherstonehaugh", "max@example.invalid"),
        )
        .email(
            EmailSpec::new("e5", "t5", &ago(400 * 24 * 60))
                .in_mailbox("mb-inbox")
                .seen()
                .from("Old Friend", "old@example.invalid")
                .subject("Catching up"),
        )
        .build()
}

/// An Inbox of `threads` single-email threads, `Subject 1` the newest.
fn numbered_inbox(threads: usize) -> Store {
    let mut builder = Store::builder().role_mailbox("mb-inbox", "Inbox", "inbox");
    for n in 1..=threads {
        builder = builder.email(
            EmailSpec::new(&format!("e{n}"), &format!("t{n}"), &ago(n as i64 * 60))
                .in_mailbox("mb-inbox")
                .from("Ana Lopez", "ana@example.invalid")
                .subject(&format!("Subject {n}")),
        );
    }
    builder.build()
}

/// blorp with a stored token, against `server`.
fn start(server: &LocalJmap, cols: u16, rows: u16) -> (Sandbox, Blorp) {
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token(TOKEN);
    let blorp = sandbox.spawn(&[], cols, rows);
    (sandbox, blorp)
}

#[tokio::test]
async fn the_inbox_loads_with_its_rows_and_statusline_at_80x24() {
    let server = LocalJmap::start(busy_inbox(), TOKEN).await;
    let (_sandbox, mut blorp) = start(&server, 80, 24);

    blorp.wait_statusline("1/5");
    blorp.keys("2j");
    blorp.wait_statusline("3/5");

    let rows = blorp.rows();
    assert_eq!(
        rows[..6],
        [
            "  2 ●  Ana Lopez            Lunch on Friday?                                  2m",
            "  1    Ravi Shah            Re: Trip to Wellington                        3   5h",
            "  3  ⚑ billing@shop.exampl… Your order 48213 has shipped and will arri…       3d",
            "  1 ●⚑ Maximiliana Feather… (no subject)                                      4w",
            "  2    Old Friend           Catching up                                       1y",
            "",
        ]
    );
    assert_eq!(
        rows[22],
        " NORMAL  Inbox                                                              3/5"
    );
    assert_eq!(rows[23], "");
}

#[tokio::test]
async fn the_inbox_loads_with_its_rows_and_statusline_at_120x40() {
    let server = LocalJmap::start(busy_inbox(), TOKEN).await;
    let (_sandbox, mut blorp) = start(&server, 120, 40);

    blorp.wait_statusline("1/5");
    blorp.keys("2j");
    blorp.wait_statusline("3/5");

    let rows = blorp.rows();
    assert_eq!(
        rows[..6],
        [
            "  2 ●  Ana Lopez            Lunch on Friday?                                                                          2m",
            "  1    Ravi Shah            Re: Trip to Wellington                                                                3   5h",
            "  3  ⚑ billing@shop.exampl… Your order 48213 has shipped and will arrive between Tuesday and Thursday next week       3d",
            "  1 ●⚑ Maximiliana Feather… (no subject)                                                                              4w",
            "  2    Old Friend           Catching up                                                                               1y",
            "",
        ]
    );
    assert_eq!(
        rows[38],
        " NORMAL  Inbox                                                                                                      3/5"
    );
    assert_eq!(rows[39], "");
}

#[tokio::test]
async fn motions_and_counts_move_the_cursor() {
    let server = LocalJmap::start(numbered_inbox(30), TOKEN).await;
    let (_sandbox, mut blorp) = start(&server, 80, 24);
    blorp.wait_statusline(" 1/30");

    for (keys, position) in [
        ("j", " 2/30"),
        ("k", " 1/30"),
        ("3j", " 4/30"),
        ("12G", " 12/30"),
        ("G", " 30/30"),
        ("g", " 1/30"),
    ] {
        blorp.keys(keys);
        blorp.wait_statusline(position);
    }
    assert!(
        blorp.rows()[0].contains("Subject 1 "),
        "the cursor row is on screen"
    );
}

#[tokio::test]
async fn esc_and_enter_drop_a_pending_count() {
    let server = LocalJmap::start(numbered_inbox(30), TOKEN).await;
    let (_sandbox, mut blorp) = start(&server, 80, 24);
    blorp.wait_statusline(" 1/30");

    for (cancel, from, to) in [("\x1b", "1/30", " 2/30"), ("\r", "2/30", " 3/30")] {
        blorp.keys("5");
        blorp.wait_statusline(&format!("5  {from}"));
        blorp.keys(cancel);
        blorp.wait_for("the count to be dropped", |screen| {
            let statusline = statusline(screen);
            statusline.ends_with(from) && !statusline.contains("5  ")
        });
        blorp.keys("j");
        blorp.wait_statusline(to);
    }
}

#[tokio::test]
async fn a_resize_redraws_at_the_new_size() {
    let server = LocalJmap::start(busy_inbox(), TOKEN).await;
    let (_sandbox, mut blorp) = start(&server, 80, 24);
    blorp.wait_statusline("1/5");

    blorp.resize(120, 40);

    blorp.wait_for("the statusline on row 39 of 40", |screen| {
        statusline(screen).ends_with("1/5")
    });
    blorp.wait_text("and Thursday next week");
}

const REJECTED: &str = "Fastmail rejected the API token. It may have been removed in Settings. \
Run `blorp setup` to store a new one.";

#[tokio::test]
async fn q_and_ctrl_c_exit_0_and_restore_the_terminal() {
    for key in ["q", "\x03"] {
        let server = LocalJmap::start(busy_inbox(), TOKEN).await;
        let (_sandbox, mut blorp) = start(&server, 80, 24);
        blorp.wait_statusline("1/5");
        assert!(blorp.on_alternate_screen(), "{key:?}");
        assert_eq!(
            blorp.modes(),
            Modes {
                canonical: false,
                echo: false,
            },
            "raw mode while running"
        );

        blorp.keys(key);
        let status = blorp.wait_exit();

        assert_eq!(status.exit_code(), 0, "{key:?}");
        assert!(!blorp.on_alternate_screen(), "{key:?}");
        assert_eq!(
            blorp.modes(),
            Modes {
                canonical: true,
                echo: true,
            },
            "{key:?}"
        );
    }
}

#[tokio::test]
async fn a_rejected_token_prints_the_error_and_exits_1_before_the_tui() {
    let server = LocalJmap::start(busy_inbox(), TOKEN).await;
    let sandbox = Sandbox::new().session_url(&server.session_url());
    sandbox.store_token("fake-token-2");
    let mut blorp = sandbox.spawn(&[], 80, 24);

    let status = blorp.wait_exit();

    assert_eq!(status.exit_code(), 1);
    assert!(blorp.output().contains(REJECTED), "{}", blorp.output());
    assert!(!blorp.entered_alternate_screen());
}

#[tokio::test]
async fn a_mailbox_larger_than_the_queue_shows_its_total() {
    let server = LocalJmap::start(numbered_inbox(600), TOKEN).await;
    let (_sandbox, blorp) = start(&server, 80, 24);

    blorp.wait_statusline(" 1/500 of 600");
}
