//! Stable opt-in error envelopes. Native payloads and command arguments are never serialized.
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use std::fmt;

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NativeError,
    RuntimeUnavailable,
    RpcTimeout,
    CommandTimeout,
    OutputLimit,
    InvalidResponse,
    Interrupted,
    InvalidArguments,
    RequestFailed,
    PreflightBlocked,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Unknown,
    Failed,
    NotStarted,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BridgeError {
    pub code: ErrorCode,
    pub message: String,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_launch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEnvelope {
    pub schema_version: u32,
    pub error: BridgeError,
}

/// Bound and escape untrusted human diagnostics without copying arbitrary error objects.
pub fn diagnostic(message: &str) -> String {
    message
        .chars()
        .take(2048)
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn identifier(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| {
            !text.is_empty()
                && text.len() <= 256
                && text
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
        })
        .map(str::to_owned)
}

impl BridgeError {
    pub fn new(code: ErrorCode, message: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            code,
            message: diagnostic(&message.into()),
            outcome,
            operation_id: None,
            retry_request_id: None,
            retry_launch_id: None,
            native_code: None,
        }
    }

    /// Preserve only known machine fields from known native error/recovery containers.
    /// In particular, never traverse params, argv, tokens, prompts or recovery commands.
    pub fn with_native(mut self, native: &Value) -> Self {
        fn visit(error: &mut BridgeError, value: &Value, depth: usize, ambiguous: &mut bool) {
            if depth > 8 {
                return;
            }
            let Some(object) = value.as_object() else {
                return;
            };
            for (key, slot) in [
                ("code", &mut error.native_code),
                ("operationId", &mut error.operation_id),
                ("retryRequestId", &mut error.retry_request_id),
                ("retryLaunchId", &mut error.retry_launch_id),
            ] {
                if slot.is_none() {
                    *slot = object.get(key).and_then(identifier);
                }
            }
            if object.get("ambiguous").and_then(Value::as_bool) == Some(true)
                || object.get("outcome").and_then(Value::as_str) == Some("unknown")
            {
                *ambiguous = true;
            }
            match object.get("outcome").and_then(Value::as_str) {
                Some("failed") => error.outcome = Outcome::Failed,
                Some("not-started") => error.outcome = Outcome::NotStarted,
                _ => (),
            }
            for container in [
                "error",
                "data",
                "details",
                "recovery",
                "receipt",
                "operation",
                "retry",
            ] {
                if let Some(child) = object.get(container) {
                    visit(error, child, depth + 1, ambiguous);
                }
            }
        }
        let mut ambiguous = false;
        visit(&mut self, native, 0, &mut ambiguous);
        if ambiguous {
            self.outcome = Outcome::Unknown;
        }
        self
    }

    /// Native terminal.send uses this replay ID; it is not the RPC correlation ID.
    /// Only call this for a proven terminal-send operation, never generic errors.
    pub fn with_prompt_recovery(mut self, native: &Value) -> Self {
        if self.retry_request_id.is_none() {
            self.retry_request_id = native
                .pointer("/error/data/orchestrationRequestId")
                .or_else(|| native.pointer("/data/orchestrationRequestId"))
                .and_then(identifier);
        }
        self
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        for (label, id) in [
            ("operationId", &self.operation_id),
            ("retryRequestId", &self.retry_request_id),
            ("retryLaunchId", &self.retry_launch_id),
        ] {
            if let Some(id) = id {
                write!(formatter, " [{label}={id}]")?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for BridgeError {}

pub fn envelope(error: &anyhow::Error) -> Value {
    let fallback;
    let typed = match error.downcast_ref::<BridgeError>() {
        Some(typed) => typed,
        None => {
            fallback = BridgeError::new(
                ErrorCode::RequestFailed,
                format!("{error:#}"),
                Outcome::Unknown,
            );
            &fallback
        }
    };
    json!(ErrorEnvelope {
        schema_version: 1,
        error: typed.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_keeps_typed_error_through_context_and_allowlists_recovery_metadata() {
        let error = BridgeError::new(ErrorCode::NativeError, "Orca rejected fixture request", Outcome::Failed)
            .with_native(&json!({"code":"fixture_native","details":{"operationId":"operation-fixture","recovery":{"retryRequestId":"request-fixture","retryLaunchId":"launch-fixture"},"authToken":"private fixture token","params":{"operationId":"private-param"}},"unknown":{"retryRequestId":"private-unknown"},"retryCommand":"private fixture command"}));
        let error = anyhow::Error::new(error).context("wrapper fixture");
        let value = envelope(&error);
        assert_eq!(value["error"]["code"], "native_error");
        assert_eq!(value["error"]["nativeCode"], "fixture_native");
        assert_eq!(value["error"]["operationId"], "operation-fixture");
        assert_eq!(value["error"]["retryRequestId"], "request-fixture");
        assert_eq!(value["error"]["retryLaunchId"], "launch-fixture");
        assert!(!value.to_string().contains("private"));
        let displayed = format!("{error:#}");
        assert!(displayed.contains("retryRequestId=request-fixture"));
        assert!(!displayed.contains("private"));
    }

    #[test]
    fn native_unknown_outcome_is_not_reported_as_a_definite_failure() {
        let error = BridgeError::new(
            ErrorCode::NativeError,
            "Fixture response uncertain",
            Outcome::Failed,
        )
        .with_native(&json!({"details":{"outcome":"unknown"}}));
        assert_eq!(envelope(&error.into())["error"]["outcome"], "unknown");
    }

    #[test]
    fn fallback_does_not_classify_diagnostic_text_or_copy_invalid_identifiers() {
        let error = anyhow::anyhow!("rpc_timeout runtime_unavailable arbitrary fixture prose");
        let value = envelope(&error);
        assert_eq!(value["error"]["code"], "request_failed");
        assert_eq!(value["error"]["outcome"], "unknown");
        let error = BridgeError::new(ErrorCode::NativeError, "fixture", Outcome::Failed)
            .with_native(&json!({"operationId":"line\nbreak","retryRequestId":"$(fixture)","code":{"token":"private"}}));
        let value = envelope(&error.into());
        assert!(value["error"].get("operationId").is_none());
        assert!(value["error"].get("retryRequestId").is_none());
        assert!(value["error"].get("nativeCode").is_none());
    }
}
