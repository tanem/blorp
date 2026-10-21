//! The keyring adapter against the platform keyring (the macOS login
//! keychain, Windows Credential Manager, or Secret Service), under a
//! throwaway service so blorp's own credential is never touched. Ignored by
//! default, since `cargo test` touches no keyring: run it with
//! `cargo test --test keyring -- --ignored`. It needs no API token.

use blorp::cli::{Keyring, TokenStore};
use secrecy::{ExposeSecret, SecretString};

/// Deletes the throwaway credential even when an assertion fails.
struct Cleanup<'a>(&'a Keyring);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

#[test]
#[ignore = "uses the platform keyring"]
fn the_keyring_stores_replaces_and_deletes_the_token() {
    let service = format!("blorp-test-{}", std::process::id());
    let keyring = Keyring::for_service(&service);
    let _cleanup = Cleanup(&keyring);

    keyring.reachable().unwrap();
    assert!(keyring.get().unwrap().is_none(), "{service} already exists");

    keyring.set(&SecretString::from("first-token")).unwrap();
    let stored = keyring.get().unwrap().expect("a stored token");
    assert_eq!(stored.expose_secret(), "first-token");

    keyring.set(&SecretString::from("second-token")).unwrap();
    let stored = keyring.get().unwrap().expect("a stored token");
    assert_eq!(stored.expose_secret(), "second-token");

    assert!(keyring.delete().unwrap());
    assert!(keyring.get().unwrap().is_none());
    assert!(
        !keyring.delete().unwrap(),
        "deleting a missing credential succeeds"
    );
}
