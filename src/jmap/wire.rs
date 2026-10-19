//! JMAP wire types for the session resource and the objects blorp reads.

use std::collections::{BTreeMap, HashMap};

use serde::de::Deserializer;
use serde::ser::{SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JMAP core capability.
pub const CORE: &str = "urn:ietf:params:jmap:core";
/// The JMAP mail capability.
pub const MAIL: &str = "urn:ietf:params:jmap:mail";

/// The session resource (RFC 8620 section 2).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub capabilities: HashMap<String, Value>,
    pub primary_accounts: HashMap<String, String>,
    pub username: String,
    pub api_url: String,
    /// The push stream's URL, a template or, as Fastmail gives it, bare.
    #[serde(default)]
    pub event_source_url: Option<String>,
}

impl Session {
    /// The primary account for mail, if the session advertises the mail
    /// capability and names one.
    pub fn mail_account(&self) -> Option<&str> {
        if !self.capabilities.contains_key(MAIL) {
            return None;
        }
        self.primary_accounts.get(MAIL).map(String::as_str)
    }

    /// The request limits the core capability advertises. A limit that is
    /// missing, or not a positive integer, takes its value in
    /// [`Limits::FASTMAIL`].
    pub fn limits(&self) -> Limits {
        let core = self.capabilities.get(CORE);
        let limit = |name: &str| {
            core.and_then(|core| core.get(name))
                .and_then(Value::as_u64)
                .filter(|&n| n > 0)
                .and_then(|n| usize::try_from(n).ok())
        };
        let default = Limits::FASTMAIL;
        Limits {
            max_objects_in_get: limit("maxObjectsInGet").unwrap_or(default.max_objects_in_get),
            max_objects_in_set: limit("maxObjectsInSet").unwrap_or(default.max_objects_in_set),
            max_calls_in_request: limit("maxCallsInRequest")
                .unwrap_or(default.max_calls_in_request),
        }
    }
}

/// The session limits blorp works within (RFC 8620 section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most ids one `/get` call may name.
    pub max_objects_in_get: usize,
    /// The most objects one `/set` call may create, update, or destroy.
    pub max_objects_in_set: usize,
    /// The most method calls one request may carry.
    pub max_calls_in_request: usize,
}

impl Limits {
    /// Fastmail's values: `maxObjectsInGet` and `maxCallsInRequest` as its
    /// session advertised them on 2026-10-19, and `maxObjectsInSet` as two
    /// published dumps of its session show it, not yet checked against a
    /// live session.
    pub const FASTMAIL: Limits = Limits {
        max_objects_in_get: 4096,
        max_objects_in_set: 4096,
        max_calls_in_request: 50,
    };
}

/// A request envelope (RFC 8620 section 3.3).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub using: Vec<String>,
    pub method_calls: Vec<Invocation>,
}

/// A response envelope (RFC 8620 section 3.4).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub method_responses: Vec<Invocation>,
}

/// One method call or response: a name, its arguments, and a call id, sent
/// as a three-element array.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub name: String,
    pub args: Value,
    pub call_id: String,
}

impl Serialize for Invocation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(3)?;
        tuple.serialize_element(&self.name)?;
        tuple.serialize_element(&self.args)?;
        tuple.serialize_element(&self.call_id)?;
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for Invocation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (name, args, call_id) = <(String, Value, String)>::deserialize(deserializer)?;
        Ok(Invocation {
            name,
            args,
            call_id,
        })
    }
}

/// A mailbox, with the properties blorp requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Option<String>,
}

/// A thread: its id and its emails' ids, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub email_ids: Vec<String>,
}

/// An email, with the properties blorp requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    pub id: String,
    pub thread_id: String,
    pub mailbox_ids: BTreeMap<String, bool>,
    pub keywords: BTreeMap<String, bool>,
    pub from: Option<Vec<EmailAddress>>,
    pub subject: Option<String>,
    pub received_at: jiff::Timestamp,
    pub has_attachment: bool,
}

/// A name and address from a header such as `From`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = r#"{
        "capabilities": {
            "urn:ietf:params:jmap:core": {"maxObjectsInGet": 4096},
            "urn:ietf:params:jmap:mail": {}
        },
        "accounts": {
            "u1a2b3c4": {"name": "kim@example.invalid", "isPersonal": true, "isReadOnly": false}
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:core": "u1a2b3c4",
            "urn:ietf:params:jmap:mail": "u1a2b3c4"
        },
        "username": "kim@example.invalid",
        "apiUrl": "https://api.example.invalid/jmap/api/",
        "downloadUrl": "https://www.example.invalid/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "https://api.example.invalid/jmap/upload/{accountId}/",
        "eventSourceUrl": "https://api.example.invalid/jmap/event/",
        "state": "cyrus-0;p-5"
    }"#;

    #[test]
    fn session_gives_the_mail_account_when_the_capability_is_advertised() {
        let session: Session = serde_json::from_str(SESSION).unwrap();

        assert_eq!(session.username, "kim@example.invalid");
        assert_eq!(session.api_url, "https://api.example.invalid/jmap/api/");
        assert_eq!(session.mail_account(), Some("u1a2b3c4"));
    }

    #[test]
    fn session_without_the_mail_capability_has_no_mail_account() {
        let session: Session = serde_json::from_str(
            r#"{
                "capabilities": {"urn:ietf:params:jmap:core": {}},
                "accounts": {},
                "primaryAccounts": {"urn:ietf:params:jmap:mail": "u1a2b3c4"},
                "username": "kim@example.invalid",
                "apiUrl": "https://api.example.invalid/jmap/api/",
                "state": "0"
            }"#,
        )
        .unwrap();

        assert_eq!(session.mail_account(), None);
    }

    #[test]
    fn session_with_the_capability_but_no_primary_mail_account_has_no_mail_account() {
        let session: Session = serde_json::from_str(
            r#"{
                "capabilities": {"urn:ietf:params:jmap:mail": {}},
                "accounts": {},
                "primaryAccounts": {},
                "username": "kim@example.invalid",
                "apiUrl": "https://api.example.invalid/jmap/api/",
                "state": "0"
            }"#,
        )
        .unwrap();

        assert_eq!(session.mail_account(), None);
    }

    fn session_with_core(core: Value) -> Session {
        serde_json::from_value(serde_json::json!({
            "capabilities": {CORE: core, MAIL: {}},
            "accounts": {},
            "primaryAccounts": {MAIL: "u1a2b3c4"},
            "username": "kim@example.invalid",
            "apiUrl": "https://api.example.invalid/jmap/api/",
            "state": "0"
        }))
        .unwrap()
    }

    #[test]
    fn session_limits_are_read_from_the_core_capability() {
        let session = session_with_core(
            serde_json::json!({"maxObjectsInGet": 256, "maxObjectsInSet": 128, "maxCallsInRequest": 16}),
        );

        assert_eq!(
            session.limits(),
            Limits {
                max_objects_in_get: 256,
                max_objects_in_set: 128,
                max_calls_in_request: 16,
            }
        );
    }

    #[test]
    fn session_limits_default_to_fastmails_when_missing() {
        let session = session_with_core(serde_json::json!({}));

        assert_eq!(
            session.limits(),
            Limits {
                max_objects_in_get: 4096,
                max_objects_in_set: 4096,
                max_calls_in_request: 50,
            }
        );
    }

    #[test]
    fn session_limits_default_to_fastmails_when_not_a_positive_integer() {
        for value in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("10"),
            Value::Null,
        ] {
            let session = session_with_core(
                serde_json::json!({"maxObjectsInGet": value, "maxObjectsInSet": value, "maxCallsInRequest": value}),
            );

            assert_eq!(
                session.limits(),
                Limits {
                    max_objects_in_get: 4096,
                    max_objects_in_set: 4096,
                    max_calls_in_request: 50,
                },
                "{value}"
            );
        }
    }

    #[test]
    fn mailbox_reads_role_and_parent() {
        let mailboxes: Vec<Mailbox> = serde_json::from_str(
            r#"[
                {"id": "mb1", "name": "Inbox", "parentId": null, "role": "inbox", "sortOrder": 1},
                {"id": "mb2", "name": "Receipts", "parentId": "mb3", "role": null, "sortOrder": 10}
            ]"#,
        )
        .unwrap();

        assert_eq!(
            mailboxes,
            [
                Mailbox {
                    id: "mb1".into(),
                    name: "Inbox".into(),
                    parent_id: None,
                    role: Some("inbox".into()),
                },
                Mailbox {
                    id: "mb2".into(),
                    name: "Receipts".into(),
                    parent_id: Some("mb3".into()),
                    role: None,
                },
            ]
        );
    }

    #[test]
    fn email_reads_mailboxes_keywords_sender_and_date() {
        let email: Email = serde_json::from_str(
            r#"{
                "id": "Mf1",
                "threadId": "Tt1",
                "mailboxIds": {"mb1": true, "mb4": true},
                "keywords": {"$seen": true, "$flagged": true},
                "from": [{"name": "Ana Lopez", "email": "ana@example.invalid"}],
                "subject": "Lunch on Friday?",
                "receivedAt": "2026-10-17T09:30:00Z",
                "hasAttachment": false
            }"#,
        )
        .unwrap();

        assert_eq!(email.id, "Mf1");
        assert_eq!(email.thread_id, "Tt1");
        assert!(email.mailbox_ids.contains_key("mb1") && email.mailbox_ids.contains_key("mb4"));
        assert!(email.keywords.contains_key("$seen") && email.keywords.contains_key("$flagged"));
        assert_eq!(
            email.from,
            Some(vec![EmailAddress {
                name: Some("Ana Lopez".into()),
                email: "ana@example.invalid".into(),
            }])
        );
        assert_eq!(email.subject.as_deref(), Some("Lunch on Friday?"));
        assert_eq!(
            email.received_at,
            "2026-10-17T09:30:00Z".parse::<jiff::Timestamp>().unwrap()
        );
        assert!(!email.has_attachment);
    }

    #[test]
    fn email_allows_a_null_sender_and_subject() {
        let email: Email = serde_json::from_str(
            r#"{
                "id": "Mf2",
                "threadId": "Tt2",
                "mailboxIds": {"mb1": true},
                "keywords": {},
                "from": null,
                "subject": null,
                "receivedAt": "2026-10-17T09:30:00Z",
                "hasAttachment": true
            }"#,
        )
        .unwrap();

        assert_eq!(email.from, None);
        assert_eq!(email.subject, None);
        assert!(email.has_attachment);
    }

    #[test]
    fn thread_reads_its_email_ids() {
        let thread: Thread =
            serde_json::from_str(r#"{"id": "Tt1", "emailIds": ["Mf0", "Mf1"]}"#).unwrap();

        assert_eq!(
            thread,
            Thread {
                id: "Tt1".into(),
                email_ids: vec!["Mf0".into(), "Mf1".into()],
            }
        );
    }

    #[test]
    fn response_reads_invocations_including_a_method_error() {
        let response: Response = serde_json::from_str(
            r#"{
                "methodResponses": [
                    ["Mailbox/get", {"accountId": "u1", "state": "1", "list": [], "notFound": []}, "0"],
                    ["error", {"type": "unknownMethod"}, "1"]
                ],
                "sessionState": "cyrus-0;p-5"
            }"#,
        )
        .unwrap();

        assert_eq!(response.method_responses.len(), 2);
        assert_eq!(response.method_responses[0].name, "Mailbox/get");
        assert_eq!(response.method_responses[0].call_id, "0");
        assert_eq!(response.method_responses[1].name, "error");
        assert_eq!(response.method_responses[1].args["type"], "unknownMethod");
    }

    #[test]
    fn request_serialises_invocations_as_arrays() {
        let request = Request {
            using: vec![CORE.into(), MAIL.into()],
            method_calls: vec![Invocation {
                name: "Mailbox/get".into(),
                args: serde_json::json!({"accountId": "u1", "ids": null}),
                call_id: "0".into(),
            }],
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
                "methodCalls": [["Mailbox/get", {"accountId": "u1", "ids": null}, "0"]]
            })
        );
    }
}
