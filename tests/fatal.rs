use blorp::app::{Effect, Msg};
use blorp::harness::Harness;
use blorp::jmap::fake::Store;
use blorp::jmap::{Error, Fatal};

fn inbox() -> Store {
    Store::builder()
        .role_mailbox("mb-inbox", "Inbox", "inbox")
        .build()
}

#[test]
fn a_rejected_token_ends_the_session() {
    for (error, fatal) in [
        (Error::Status(401), Fatal::TokenRejected),
        (Error::Status(403), Fatal::NoEmailScope),
    ] {
        let mut h = Harness::new(inbox());
        h.start();

        let effects = h.send(Msg::Loaded {
            generation: 1,
            result: Err(error.clone()),
        });

        assert_eq!(effects, [Effect::Fatal(fatal)], "{error:?}");
    }
}

#[test]
fn a_rejected_token_ends_the_session_even_on_a_superseded_query() {
    let mut h = Harness::new(inbox());
    h.start();
    h.start();

    let effects = h.send(Msg::Loaded {
        generation: 1,
        result: Err(Error::Status(401)),
    });

    assert_eq!(effects, [Effect::Fatal(Fatal::TokenRejected)]);
}
