//! A local JMAP server for tests: the session resource and the API, served
//! from a fake store on a loopback port.

use std::sync::Arc;

use blorp::jmap::fake::{MAX_OBJECTS_IN_GET, Store, api};
use blorp::jmap::{CORE, MAIL};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The account id the session names for mail.
pub const ACCOUNT_ID: &str = "u1a2b3c4";
/// The username the session reports.
pub const USERNAME: &str = "kim@example.invalid";

/// A JMAP server on localhost that accepts one token.
pub struct LocalJmap {
    server: MockServer,
}

impl LocalJmap {
    /// Serves `store` to requests bearing `token`.
    pub async fn start(store: Store, token: &str) -> LocalJmap {
        LocalJmap::serve(store, token, true).await
    }

    /// Serves a session that does not advertise the mail capability.
    pub async fn start_without_mail(token: &str) -> LocalJmap {
        LocalJmap::serve(Store::builder().build(), token, false).await
    }

    async fn serve(store: Store, token: &str, mail: bool) -> LocalJmap {
        let server = MockServer::start().await;
        let auth = format!("Bearer {token}");
        let session = session(&server.uri(), mail);
        Mock::given(method("GET"))
            .and(path("/jmap/session"))
            .respond_with(Authorized {
                auth: auth.clone(),
                answer: Box::new(move |_| ResponseTemplate::new(200).set_body_json(&session)),
            })
            .mount(&server)
            .await;
        let store = Arc::new(store);
        Mock::given(method("POST"))
            .and(path("/jmap/api/"))
            .respond_with(Authorized {
                auth,
                answer: Box::new(move |request| api_response(&store, request)),
            })
            .mount(&server)
            .await;
        LocalJmap { server }
    }

    pub fn session_url(&self) -> String {
        format!("{}/jmap/session", self.server.uri())
    }
}

type Answer = Box<dyn Fn(&Request) -> ResponseTemplate + Send + Sync>;

/// Answers requests bearing the token, and 401 to any other.
struct Authorized {
    auth: String,
    answer: Answer,
}

impl Respond for Authorized {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let authorized = request
            .headers
            .get("authorization")
            .is_some_and(|value| value.as_bytes() == self.auth.as_bytes());
        if !authorized {
            return ResponseTemplate::new(401);
        }
        (self.answer)(request)
    }
}

fn api_response(store: &Store, request: &Request) -> ResponseTemplate {
    let result = serde_json::from_slice::<Value>(&request.body)
        .map_err(|_| "urn:ietf:params:jmap:error:notJSON".to_string())
        .and_then(|body| api::respond(store, ACCOUNT_ID, &body));
    match result {
        Ok(response) => ResponseTemplate::new(200).set_body_json(response),
        Err(kind) => ResponseTemplate::new(400).set_body_json(json!({"type": kind, "status": 400})),
    }
}

/// A session resource in Fastmail's shape, advertising Fastmail's
/// `maxObjectsInGet`.
fn session(base: &str, mail: bool) -> Value {
    let mut capabilities = json!({CORE: {"maxObjectsInGet": MAX_OBJECTS_IN_GET}});
    let mut primary = json!({CORE: ACCOUNT_ID});
    if mail {
        capabilities[MAIL] = json!({});
        primary[MAIL] = json!(ACCOUNT_ID);
    }
    json!({
        "capabilities": capabilities,
        "accounts": {ACCOUNT_ID: {"name": USERNAME, "isPersonal": true, "isReadOnly": false}},
        "primaryAccounts": primary,
        "username": USERNAME,
        "apiUrl": format!("{base}/jmap/api/"),
        "state": "0"
    })
}
