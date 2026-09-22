use crate::{lists, messaging::MAX_OUTGOING_BYTES, rules};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use std::io;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    id: String,
    #[serde(rename = "type")]
    kind: String,
    payload: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyPayload {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilePayload {
    text: String,
    #[serde(default = "default_source")]
    source: String,
}

fn default_source() -> String {
    "Local import".into()
}

#[derive(Debug, Serialize)]
pub struct Response {
    version: u32,
    id: Option<String>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ProtocolError>,
}

#[derive(Debug, Serialize)]
struct ProtocolError {
    code: &'static str,
    message: String,
}

impl Response {
    fn success(id: String, payload: Value) -> Self {
        Self {
            version: 1,
            id: Some(id),
            ok: true,
            payload: Some(payload),
            error: None,
        }
    }

    fn failure(id: Option<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            version: 1,
            id,
            ok: false,
            payload: None,
            error: Some(ProtocolError {
                code,
                message: message.into(),
            }),
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
}

pub fn handle(bytes: &[u8]) -> Response {
    // The Value pass only recovers a valid correlation ID. Validation uses the
    // original JSON so duplicate fields cannot be hidden by Value's map parsing.
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => {
            return Response::failure(None, "INVALID_JSON", "Message must be valid UTF-8 JSON")
        }
    };
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .map(str::to_owned);
    if !value.is_object() || !value.get("payload").is_some_and(Value::is_object) {
        return Response::failure(
            id,
            "INVALID_REQUEST",
            "Request and payload must be JSON objects",
        );
    }
    let request: Request = match serde_json::from_slice(bytes) {
        Ok(request) => request,
        Err(_) => {
            return Response::failure(
                id,
                "INVALID_REQUEST",
                "Expected exactly version, id, type, and payload fields",
            )
        }
    };
    if !valid_id(&request.id) {
        return Response::failure(
            None,
            "INVALID_REQUEST",
            "id must contain 1 to 128 UTF-8 bytes",
        );
    }
    let id = Some(request.id.clone());
    if request.version != 1 {
        return Response::failure(
            id,
            "UNSUPPORTED_VERSION",
            "Only protocol version 1 is supported",
        );
    }
    match request.kind.as_str() {
        "status.get" => {
            if serde_json::from_str::<EmptyPayload>(request.payload.get()).is_err() {
                return Response::failure(
                    id,
                    "INVALID_REQUEST",
                    "status.get payload must be an empty object",
                );
            }
            Response::success(
                request.id,
                json!({
                    "companionVersion": env!("CARGO_PKG_VERSION"),
                    "protocolVersion": 1,
                    "healthy": true,
                    "capabilities": ["rules.compile", "lists.refresh", "lists.page"],
                    "mode": "local-import"
                }),
            )
        }
        "rules.compile" => {
            let payload: CompilePayload = match serde_json::from_str(request.payload.get()) {
                Ok(payload) => payload,
                Err(_) => {
                    return Response::failure(
                        id,
                        "INVALID_REQUEST",
                        "rules.compile requires text and optional source strings only",
                    )
                }
            };
            if payload.source.len() > 128 {
                return Response::failure(
                    id,
                    "INVALID_REQUEST",
                    "source must not exceed 128 UTF-8 bytes",
                );
            }
            match rules::compile(&payload.text, &payload.source) {
                Ok(compilation) => Response::success(request.id, json!(compilation)),
                Err(error) => Response::failure(id, "LIMIT_EXCEEDED", error),
            }
        }
        "lists.refresh" => {
            let payload: lists::RefreshRequest = match serde_json::from_str(request.payload.get()) {
                Ok(payload) => payload,
                Err(_) => {
                    return Response::failure(
                        id,
                        "INVALID_REQUEST",
                        "lists.refresh requires only ids and networkBudget",
                    )
                }
            };
            match lists::refresh(payload) {
                Ok(manifest) => Response::success(request.id, manifest),
                Err(error) => Response::failure(id, error.code, error.message),
            }
        }
        "lists.page" => {
            let payload: lists::PageRequest = match serde_json::from_str(request.payload.get()) {
                Ok(payload) => payload,
                Err(_) => {
                    return Response::failure(
                        id,
                        "INVALID_REQUEST",
                        "lists.page requires only snapshotId, kind, and offset",
                    )
                }
            };
            match lists::page(payload) {
                Ok(page) => Response::success(request.id, page),
                Err(error) => Response::failure(id, error.code, error.message),
            }
        }
        _ => Response::failure(id, "UNKNOWN_MESSAGE_TYPE", "Unknown message type"),
    }
}

pub fn encode_response(response: Response) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(&response).map_err(io::Error::other)?;
    if bytes.len() <= MAX_OUTGOING_BYTES {
        return Ok(bytes);
    }
    serde_json::to_vec(&Response::failure(
        response.id,
        "OUTPUT_TOO_LARGE",
        "Compilation exceeds the 1 MiB response limit; import a smaller list",
    ))
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: &str, payload: Value) -> Value {
        let input = json!({"version": 1, "id": "test", "type": kind, "payload": payload});
        serde_json::to_value(handle(&serde_json::to_vec(&input).unwrap())).unwrap()
    }

    #[test]
    fn status_reports_truthful_local_capabilities() {
        let response = request("status.get", json!({}));
        assert_eq!(response["ok"], true);
        assert_eq!(response["payload"]["healthy"], true);
        assert_eq!(response["payload"]["mode"], "local-import");
        assert_eq!(
            response["payload"]["capabilities"],
            json!(["rules.compile", "lists.refresh", "lists.page"])
        );
    }

    #[test]
    fn malformed_envelopes_fail_with_structured_errors() {
        for input in [
            "null",
            "[]",
            "{}",
            r#"{"version":1,"id":"x","type":"status.get","payload":{},"extra":0}"#,
            r#"{"version":1,"version":1,"id":"x","type":"status.get","payload":{}}"#,
            r#"{"version":1,"id":4,"type":"status.get","payload":{}}"#,
        ] {
            let response = serde_json::to_value(handle(input.as_bytes())).unwrap();
            assert_eq!(response["error"]["code"], "INVALID_REQUEST", "{input}");
            assert_eq!(response["ok"], false);
            assert!(response.get("payload").is_none());
        }
        for bytes in [b"{".as_slice(), b"\xff", b""] {
            let response = serde_json::to_value(handle(bytes)).unwrap();
            assert_eq!(response["error"]["code"], "INVALID_JSON");
            assert!(response["id"].is_null());
        }
    }

    #[test]
    fn duplicate_payload_fields_are_rejected() {
        let input = br#"{"version":1,"id":"x","type":"rules.compile","payload":{"text":"","text":"||example.test^"}}"#;
        let response = serde_json::to_value(handle(input)).unwrap();
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
    }

    #[test]
    fn payloads_are_strict_objects() {
        for payload in [
            json!(null),
            json!([]),
            json!({"extra": true}),
            json!({"text": ""}),
        ] {
            assert_eq!(
                request("status.get", payload)["error"]["code"],
                "INVALID_REQUEST"
            );
        }
        for payload in [
            json!({}),
            json!({"text": 7}),
            json!({"text":"", "path":"/tmp/list"}),
            json!({"text":"", "source":null}),
        ] {
            assert_eq!(
                request("rules.compile", payload)["error"]["code"],
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn id_limits_use_utf8_bytes_and_invalid_id_is_not_echoed() {
        for id in ["".into(), "a".repeat(129), "é".repeat(65)] {
            let input = json!({"version":1,"id":id,"type":"status.get","payload":{}});
            let response =
                serde_json::to_value(handle(&serde_json::to_vec(&input).unwrap())).unwrap();
            assert_eq!(response["error"]["code"], "INVALID_REQUEST");
            assert!(response["id"].is_null());
        }
        let input = json!({"version":1,"id":"é".repeat(64),"type":"status.get","payload":{}});
        assert!(handle(&serde_json::to_vec(&input).unwrap()).ok);
    }

    #[test]
    fn unknown_version_and_type_have_distinct_codes() {
        let input = br#"{"version":2,"id":"x","type":"status.get","payload":{}}"#;
        let response = serde_json::to_value(handle(input)).unwrap();
        assert_eq!(response["error"]["code"], "UNSUPPORTED_VERSION");
        assert_eq!(response["id"], "x");
        assert_eq!(
            request("do.anything", json!({}))["error"]["code"],
            "UNKNOWN_MESSAGE_TYPE"
        );
    }

    #[test]
    fn compile_checks_source_and_input_limits() {
        assert_eq!(
            request("rules.compile", json!({"text":"", "source":"é".repeat(65)}))["error"]["code"],
            "INVALID_REQUEST"
        );
        assert_eq!(
            request(
                "rules.compile",
                json!({"text":"a".repeat(rules::MAX_TEXT_BYTES + 1)})
            )["error"]["code"],
            "LIMIT_EXCEEDED"
        );
        let response = request("rules.compile", json!({"text":"example.test##.ad"}));
        assert_eq!(
            response["payload"]["cosmeticRules"][0]["source"],
            "Local import"
        );
    }

    #[test]
    fn large_responses_become_bounded_errors_with_correlation_id() {
        let response = Response::success(
            "same".into(),
            json!({"text":"x".repeat(MAX_OUTGOING_BYTES)}),
        );
        let bytes = encode_response(response).unwrap();
        assert!(bytes.len() < 1024);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["id"], "same");
        assert_eq!(value["error"]["code"], "OUTPUT_TOO_LARGE");
    }

    #[test]
    fn subscription_payloads_are_strict_and_reject_paths_without_network_io() {
        for payload in [
            json!({}),
            json!({"ids":["easylist"],"networkBudget":1,"url":"https://other.test"}),
            json!({"ids":["easylist"],"networkBudget":-1}),
            json!({"ids":["easylist"],"networkBudget":1.5}),
            json!({"ids":["easylist"],"networkBudget":29801}),
            json!({"ids":["easylist","easylist"],"networkBudget":1}),
            json!({"ids":["https://other.test"],"networkBudget":1}),
        ] {
            assert_eq!(
                request("lists.refresh", payload)["error"]["code"],
                "INVALID_REQUEST"
            );
        }
        for payload in [
            json!({"snapshotId":"../file","kind":"network","offset":0}),
            json!({"snapshotId":"a".repeat(64),"kind":"other","offset":0}),
            json!({"snapshotId":"a".repeat(64),"kind":"network","offset":-1}),
            json!({"snapshotId":"a".repeat(64),"kind":"network","offset":0,"path":"file"}),
        ] {
            assert_eq!(
                request("lists.page", payload)["error"]["code"],
                "INVALID_REQUEST"
            );
        }
        let duplicate = br#"{"version":1,"id":"x","type":"lists.refresh","payload":{"ids":["easylist"],"ids":["easyprivacy"],"networkBudget":1}}"#;
        assert_eq!(
            serde_json::to_value(handle(duplicate)).unwrap()["error"]["code"],
            "INVALID_REQUEST"
        );
    }
}
