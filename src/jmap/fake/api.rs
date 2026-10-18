//! A [`Store`] answering JMAP requests as JSON, for a local test server: the
//! request envelope, back-references, and the methods blorp sends.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::Store;
use crate::jmap::{CORE, EmailQuery, Error, MAIL};

/// Answers a JMAP request envelope with its response envelope. A
/// request-level error is returned as its problem details, to be sent with
/// HTTP 400 (RFC 8620 section 3.6.1). A request of more than
/// `max_calls_in_request` method calls is refused with the `limit` error.
pub fn respond(
    store: &Store,
    account_id: &str,
    max_calls_in_request: usize,
    request: &Value,
) -> Result<Value, Value> {
    let using: Vec<&str> = request["using"]
        .as_array()
        .ok_or_else(not_request)?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if !using.contains(&CORE) || !using.contains(&MAIL) {
        return Err(problem("urn:ietf:params:jmap:error:unknownCapability"));
    }
    let calls = request["methodCalls"].as_array().ok_or_else(not_request)?;
    if calls.len() > max_calls_in_request {
        let mut limit = problem("urn:ietf:params:jmap:error:limit");
        limit["limit"] = json!("maxCallsInRequest");
        return Err(limit);
    }
    let mut responses: Vec<Value> = Vec::new();
    for call in calls {
        let (Some(name), Some(args), Some(call_id)) =
            (call[0].as_str(), call[1].as_object(), call[2].as_str())
        else {
            return Err(not_request());
        };
        let answer = resolve(args, &responses)
            .and_then(|args| method(store, account_id, name, &args))
            .map(|args| (name, args))
            .unwrap_or_else(|kind| ("error", json!({"type": kind})));
        responses.push(json!([answer.0, answer.1, call_id]));
    }
    Ok(json!({"methodResponses": responses, "sessionState": "0"}))
}

/// A request-level error's problem details.
pub fn problem(kind: &str) -> Value {
    json!({"type": kind, "status": 400})
}

fn not_request() -> Value {
    problem("urn:ietf:params:jmap:error:notRequest")
}

/// Replaces each `#name` argument with the value its result reference
/// points at (RFC 8620 section 3.7).
fn resolve(args: &Map<String, Value>, responses: &[Value]) -> Result<Map<String, Value>, String> {
    let mut resolved = Map::new();
    for (key, value) in args {
        let Some(name) = key.strip_prefix('#') else {
            resolved.insert(key.clone(), value.clone());
            continue;
        };
        if args.contains_key(name) {
            return Err("invalidArguments".into());
        }
        let invalid = || "invalidResultReference".to_string();
        let (Some(result_of), Some(method), Some(path)) = (
            value["resultOf"].as_str(),
            value["name"].as_str(),
            value["path"].as_str(),
        ) else {
            return Err(invalid());
        };
        let response = responses
            .iter()
            .find(|r| r[2] == result_of && r[0] == method)
            .ok_or_else(invalid)?;
        let tokens: Vec<String> = path
            .strip_prefix('/')
            .ok_or_else(invalid)?
            .split('/')
            .map(|t| t.replace("~1", "/").replace("~0", "~"))
            .collect();
        let target = pointer(&response[1], &tokens).ok_or_else(invalid)?;
        resolved.insert(name.to_string(), target);
    }
    Ok(resolved)
}

/// A JSON pointer in which `*` maps over an array, flattening any arrays
/// the rest of the path yields.
fn pointer(value: &Value, tokens: &[String]) -> Option<Value> {
    let Some((first, rest)) = tokens.split_first() else {
        return Some(value.clone());
    };
    match value {
        Value::Array(items) if first == "*" => {
            let mut out = Vec::new();
            for item in items {
                match pointer(item, rest)? {
                    Value::Array(inner) => out.extend(inner),
                    other => out.push(other),
                }
            }
            Some(Value::Array(out))
        }
        Value::Array(items) => pointer(items.get(first.parse::<usize>().ok()?)?, rest),
        Value::Object(map) => pointer(map.get(first)?, rest),
        _ => None,
    }
}

/// Runs one method call, returning its response arguments or an error type.
fn method(
    store: &Store,
    account_id: &str,
    name: &str,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    if args.get("accountId").and_then(Value::as_str) != Some(account_id) {
        return Err("accountNotFound".into());
    }
    let ids = || ids(args);
    match name {
        "Mailbox/get" => {
            let ids = ids()?;
            let (list, not_found) = store.get_mailboxes(ids.as_deref()).map_err(kind)?;
            get_response(account_id, &list, &not_found, args)
        }
        "Email/get" => {
            let ids = ids()?.ok_or("invalidArguments")?;
            let (list, not_found) = store.get_emails(&ids).map_err(kind)?;
            get_response(account_id, &list, &not_found, args)
        }
        "Thread/get" => {
            let ids = ids()?.ok_or("invalidArguments")?;
            let (list, not_found) = store.get_threads(&ids).map_err(kind)?;
            get_response(account_id, &list, &not_found, args)
        }
        "Email/query" => email_query(store, account_id, args),
        _ => Err("unknownMethod".into()),
    }
}

/// The `ids` argument: `None` for null or absent.
fn ids(args: &Map<String, Value>) -> Result<Option<Vec<String>>, String> {
    match args.get("ids") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| id.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .map(Some)
            .ok_or_else(|| "invalidArguments".into()),
        Some(_) => Err("invalidArguments".into()),
    }
}

fn kind(error: Error) -> String {
    match error {
        Error::Method { kind, .. } => kind,
        other => panic!("the fake store failed outside JMAP: {other}"),
    }
}

/// A `/get` response, each object cut to the requested `properties`, with
/// `id` always included.
fn get_response<T: Serialize>(
    account_id: &str,
    list: &[T],
    not_found: &[String],
    args: &Map<String, Value>,
) -> Result<Value, String> {
    let properties: Option<HashSet<&str>> = match args.get("properties") {
        None | Some(Value::Null) => None,
        Some(Value::Array(names)) => Some(names.iter().filter_map(Value::as_str).collect()),
        Some(_) => return Err("invalidArguments".into()),
    };
    let list: Vec<Value> = list
        .iter()
        .map(|object| {
            let mut value = serde_json::to_value(object).expect("wire types serialise");
            if let (Some(properties), Value::Object(map)) = (&properties, &mut value) {
                map.retain(|key, _| key == "id" || properties.contains(key.as_str()));
            }
            value
        })
        .collect();
    Ok(json!({"accountId": account_id, "state": "0", "list": list, "notFound": not_found}))
}

/// `Email/query`, sorted by `receivedAt` with threads collapsed, which is
/// the only form the store evaluates.
fn email_query(
    store: &Store,
    account_id: &str,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    let ascending = match args
        .get("sort")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([sort]) if sort["property"] == "receivedAt" => sort
            .get("isAscending")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        _ => return Err("unsupportedSort".into()),
    };
    if args.get("collapseThreads") != Some(&Value::Bool(true)) {
        return Err("invalidArguments".into());
    }
    let limit = match args.get("limit") {
        None => usize::MAX,
        Some(limit) => limit.as_u64().ok_or("invalidArguments")? as usize,
    };
    let query = EmailQuery {
        filter: args.get("filter").cloned().unwrap_or(json!({})),
        ascending,
        limit,
    };
    let (ids, total) = store.query_ids(&query).map_err(kind)?;
    let mut response = json!({
        "accountId": account_id,
        "queryState": "0",
        "canCalculateChanges": false,
        "position": 0,
        "ids": ids,
    });
    if args.get("calculateTotal") == Some(&Value::Bool(true)) {
        response["total"] = json!(total);
    }
    Ok(response)
}
