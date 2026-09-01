//! The JSON-RPC 2.0 message envelope: the generic layer under any Clatch control
//! channel. A loose [`Frame`] parses any message, then [`Frame::classify`] sorts
//! it into request / notification / response by which fields are present. Method
//! names, param payloads, and error codes are the vocabulary's job, not this one.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC: &str = "2.0";

/// JSON-RPC request id. One id space per direction; a [`Response`] always answers
/// a request the sender made, so the two directions never collide.
pub type Id = u64;

/// A request: `id` + `method` + `params`, expecting a [`Response`].
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    pub params: Value,
}

/// A fire-and-forget notification: `method` + `params`, no `id`, no reply.
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

/// A reply to a request: exactly one of `result` / `error`.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Request {
    pub fn new(id: Id, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            method: method.into(),
            params,
        }
    }
}

impl Notification {
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            method: method.into(),
            params,
        }
    }
}

impl Response {
    pub fn ok(id: Id, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Id, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// A JSON-RPC error object. The numeric codes belong to each channel's vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// One frame off the wire before its kind is known. JSON-RPC distinguishes the
/// three by which fields are present, so we parse loosely then [`classify`].
///
/// [`classify`]: Frame::classify
#[derive(Debug, Deserialize)]
pub struct Frame {
    #[serde(default)]
    pub id: Option<Id>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

/// What a [`Frame`] turned out to be.
pub enum Kind {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Frame {
    /// Sort a frame by the JSON-RPC rule: a method with an id is a request, a
    /// method without an id a notification, an id without a method a response.
    /// Anything else is malformed.
    pub fn classify(self) -> Option<Kind> {
        match (self.method, self.id) {
            (Some(method), Some(id)) => Some(Kind::Request(Request {
                jsonrpc: JSONRPC.into(),
                id,
                method,
                params: self.params,
            })),
            (Some(method), None) => Some(Kind::Notification(Notification {
                jsonrpc: JSONRPC.into(),
                method,
                params: self.params,
            })),
            (None, Some(id)) => Some(Kind::Response(Response {
                jsonrpc: JSONRPC.into(),
                id,
                result: self.result,
                error: self.error,
            })),
            (None, None) => None,
        }
    }
}
