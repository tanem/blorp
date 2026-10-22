//! A local JMAP server holding a made-up Inbox, for recording the demo GIF
//! with `vhs demo/blorp.tape`. Serves on 127.0.0.1:4848 until it is
//! stopped, to requests bearing the token `demo-token`.

use std::net::TcpListener;

use blorp::jmap::fake::{EmailSpec, Store};
use jiff::{SignedDuration, Timestamp};

#[allow(dead_code)]
#[path = "../tests/e2e/server.rs"]
mod server;

use server::LocalJmap;

const ADDRESS: &str = "127.0.0.1:4848";
const TOKEN: &str = "demo-token";

/// RFC 3339 for `minutes` before now, so the ages on screen are the same
/// whenever the demo is recorded.
fn ago(minutes: i64) -> String {
    (Timestamp::now() - SignedDuration::from_mins(minutes)).to_string()
}

/// An email in the Inbox from `name` at `address`.
fn email(id: &str, thread: &str, minutes: i64, name: &str, address: &str) -> EmailSpec {
    EmailSpec::new(id, thread, &ago(minutes))
        .in_mailbox("mb-inbox")
        .from(name, address)
}

fn inbox() -> Store {
    const HOUR: i64 = 60;
    const DAY: i64 = 24 * HOUR;
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .role_mailbox("mb-archive", "Archive", "archive")
        .email(email("e1", "t1", 3, "Ana Lopez", "ana@example.invalid").subject("Lunch on Friday?"))
        .email(
            email("e2", "t2", 40, "Build bot", "ci@example.invalid")
                .subject("Nightly build passed"),
        )
        .email(
            email("e3a", "t3", 6 * HOUR, "Kim Park", "kim@example.invalid")
                .seen()
                .subject("Trip to Wellington"),
        )
        .email(
            email("e3b", "t3", 2 * HOUR, "Ravi Shah", "ravi@example.invalid")
                .seen()
                .subject("Re: Trip to Wellington"),
        )
        .email(
            email("e3c", "t3", HOUR + 15, "Kim Park", "kim@example.invalid")
                .subject("Re: Trip to Wellington"),
        )
        .email(
            email("e4", "t4", 5 * HOUR, "", "billing@shop.example.invalid")
                .seen()
                .flagged()
                .subject("Your order 48213 has shipped"),
        )
        .email(
            email("e5", "t5", DAY + 3 * HOUR, "Sam Ito", "sam@example.invalid")
                .seen()
                .subject("Notes from the planning meeting"),
        )
        .email(
            email("e6a", "t6", 2 * DAY, "Lee Chen", "lee@example.invalid")
                .seen()
                .subject("Choir rehearsal moved"),
        )
        .email(
            email("e6b", "t6", 2 * DAY - 30, "Mia Ross", "mia@example.invalid")
                .seen()
                .subject("Re: Choir rehearsal moved"),
        )
        .email(
            email(
                "e7",
                "t7",
                3 * DAY,
                "City Library",
                "library@example.invalid",
            )
            .seen()
            .flagged()
            .subject("A book you reserved is ready"),
        )
        .email(
            email("e8", "t8", 6 * DAY, "Noah Webb", "noah@example.invalid")
                .seen()
                .subject("Photos from the weekend"),
        )
        .email(
            email(
                "e9",
                "t9",
                12 * DAY,
                "Power Co",
                "accounts@power.example.invalid",
            )
            .seen()
            .subject("Your October bill"),
        )
        .build()
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind(ADDRESS).unwrap_or_else(|e| panic!("bind {ADDRESS}: {e}"));
    let server = LocalJmap::start_on(listener, inbox(), TOKEN).await;
    println!("{}", server.session_url());
    std::future::pending::<()>().await;
}
