//! Shared protocol errors (`spec/conventions.md` §Shared errors).

use serde_json::{Value as Json, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    InvalidArgument,
    UnsupportedVersion,
    Unauthenticated,
    PermissionDenied,
    NotFound,
    Conflict,
    ResourceExhausted,
    DeadlineExceeded,
    Cancelled,
    Unavailable,
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::InvalidArgument => "invalid_argument",
            Code::UnsupportedVersion => "unsupported_version",
            Code::Unauthenticated => "unauthenticated",
            Code::PermissionDenied => "permission_denied",
            Code::NotFound => "not_found",
            Code::Conflict => "conflict",
            Code::ResourceExhausted => "resource_exhausted",
            Code::DeadlineExceeded => "deadline_exceeded",
            Code::Cancelled => "cancelled",
            Code::Unavailable => "unavailable",
            Code::Internal => "internal",
        }
    }
}

/// A shared error: never carries stack traces, credentials or private state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub code: Code,
    pub message: String,
    pub retryable: bool,
}

impl ApiError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        let retryable = matches!(code, Code::Unavailable | Code::ResourceExhausted);
        let mut message = message.into();
        if message.is_empty() {
            message = code.as_str().to_owned();
        }
        Self {
            code,
            message,
            retryable,
        }
    }
    pub fn invalid(m: impl Into<String>) -> Self {
        Self::new(Code::InvalidArgument, m)
    }
    pub fn denied(m: impl Into<String>) -> Self {
        Self::new(Code::PermissionDenied, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(Code::NotFound, m)
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(Code::Conflict, m)
    }
    pub fn exhausted(m: impl Into<String>) -> Self {
        Self::new(Code::ResourceExhausted, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(Code::Unavailable, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(Code::Internal, m)
    }
    pub fn to_json(&self) -> Json {
        json!({"code": self.code.as_str(), "message": self.message, "retryable": self.retryable})
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}
impl std::error::Error for ApiError {}
