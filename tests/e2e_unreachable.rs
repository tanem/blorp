//! The blorp binary in a pseudo-terminal when Fastmail cannot be reached.

use std::time::Duration;

mod e2e;

use e2e::{Sandbox, TOKEN};

#[test]
fn a_session_fetch_that_fails_despite_retries_prints_the_error_and_exits_1() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/jmap/session", listener.local_addr().unwrap());
    drop(listener);
    let sandbox = Sandbox::new().session_url(&url);
    sandbox.store_token(TOKEN);
    let mut blorp = sandbox.spawn(&[], 80, 24);

    // Three retries, 1, 2, and 4 s apart, each with up to 500 ms of jitter.
    let status = blorp.wait_exit_within(Duration::from_secs(30));

    assert_eq!(status.exit_code(), 1);
    assert!(
        blorp.output().contains("Could not reach Fastmail: "),
        "{}",
        blorp.output()
    );
    assert!(!blorp.entered_alternate_screen());
}
