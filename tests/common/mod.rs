//! Wire fixtures shared by the tests that run against a mock server.

use serde_json::{Value, json};
use wiremock::MockServer;

/// A session resource in Fastmail's shape, pointing at `server`.
pub fn session(server: &MockServer, capabilities: Value) -> Value {
    json!({
        "capabilities": capabilities,
        "accounts": {"u1a2b3c4": {"name": "kim@example.invalid", "isPersonal": true}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "u1a2b3c4"},
        "username": "kim@example.invalid",
        "apiUrl": format!("{}/api/", server.uri()),
        "eventSourceUrl": format!("{}/event/", server.uri()),
        "state": "0"
    })
}

/// The core and mail capabilities.
pub fn mail_capabilities() -> Value {
    json!({"urn:ietf:params:jmap:core": {}, "urn:ietf:params:jmap:mail": {}})
}
