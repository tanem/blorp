//! The keychain adapter against the real login keychain, under a throwaway
//! service so blorp's own item is never touched. Ignored by default, since
//! `cargo test` touches no keychain: run it with
//! `cargo test --test keychain -- --ignored`. It needs no API token.

use blorp::cli::{Keychain, TokenStore};
use secrecy::{ExposeSecret, SecretString};

/// Deletes the throwaway item even when an assertion fails.
struct Cleanup<'a>(&'a Keychain);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

#[test]
#[ignore = "uses the real login keychain"]
fn the_keychain_stores_replaces_and_deletes_the_token() {
    let service = format!("blorp-test-{}", std::process::id());
    let keychain = Keychain::for_service(&service).unwrap();
    let _cleanup = Cleanup(&keychain);

    assert!(
        keychain.get().unwrap().is_none(),
        "{service} already exists"
    );

    keychain.set(&SecretString::from("first-token")).unwrap();
    let stored = keychain.get().unwrap().expect("a stored token");
    assert_eq!(stored.expose_secret(), "first-token");

    keychain.set(&SecretString::from("second-token")).unwrap();
    let stored = keychain.get().unwrap().expect("a stored token");
    assert_eq!(stored.expose_secret(), "second-token");

    assert!(keychain.delete().unwrap());
    assert!(keychain.get().unwrap().is_none());
    assert!(
        !keychain.delete().unwrap(),
        "deleting a missing item succeeds"
    );
}
