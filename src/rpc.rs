use crate::{
    config::Config,
    errors::{BridgeError, ErrorCode, Outcome, diagnostic},
};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const MAX_FRAME: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    runtime_id: String,
    auth_token: String,
    #[serde(default)]
    transports: Vec<Transport>,
    transport: Option<Transport>,
}

#[derive(Deserialize)]
struct Transport {
    kind: String,
    endpoint: String,
}

#[derive(Clone)]
pub struct RpcClient {
    metadata_path: PathBuf,
    timeout: Duration,
}

impl RpcClient {
    pub fn new(config: &Config) -> Self {
        Self {
            metadata_path: config.runtime_metadata(),
            timeout: Duration::from_secs(10),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let bytes = std::fs::read(&self.metadata_path).map_err(|_| {
            BridgeError::new(
                ErrorCode::RuntimeUnavailable,
                format!(
                    "Orca runtime metadata unavailable at {}",
                    self.metadata_path.display()
                ),
                Outcome::NotStarted,
            )
        })?;
        let metadata: Metadata = serde_json::from_slice(&bytes).map_err(|_| {
            BridgeError::new(
                ErrorCode::InvalidResponse,
                "Invalid Orca runtime metadata",
                Outcome::NotStarted,
            )
        })?;
        let transport = metadata
            .transports
            .iter()
            .chain(metadata.transport.iter())
            .find(|t| t.kind == "unix")
            .ok_or_else(|| {
                BridgeError::new(
                    ErrorCode::RuntimeUnavailable,
                    "Orca has no Unix socket transport",
                    Outcome::NotStarted,
                )
            })?;
        let mut socket = UnixStream::connect(&transport.endpoint).map_err(|_| {
            BridgeError::new(
                ErrorCode::RuntimeUnavailable,
                "Cannot connect to Orca; is it running?",
                Outcome::NotStarted,
            )
        })?;
        socket.set_read_timeout(Some(self.timeout)).map_err(|_| {
            BridgeError::new(
                ErrorCode::RuntimeUnavailable,
                "Cannot configure Orca RPC read timeout",
                Outcome::NotStarted,
            )
        })?;
        socket.set_write_timeout(Some(self.timeout)).map_err(|_| {
            BridgeError::new(
                ErrorCode::RuntimeUnavailable,
                "Cannot configure Orca RPC write timeout",
                Outcome::NotStarted,
            )
        })?;
        let id = uuid::Uuid::new_v4().to_string();
        let request =
            json!({"id":id,"authToken":metadata.auth_token,"method":method,"params":params});
        serde_json::to_writer(&mut socket, &request)
            .map_err(|error| transport_error(method, error.io_error_kind()))?;
        socket
            .write_all(b"\n")
            .map_err(|error| transport_error(method, Some(error.kind())))?;
        let mut reader = BufReader::new(socket);
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let mut line = Vec::new();
            loop {
                if crate::events::STOP.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(BridgeError::new(
                        ErrorCode::Interrupted,
                        "Orca RPC interrupted; request outcome is unknown",
                        Outcome::Unknown,
                    )
                    .into());
                }
                // A socket timeout bounds one read, not the entire response. Refresh
                // it from the same deadline for every fragment and keepalive frame.
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(timeout_error(method).into());
                }
                if reader.buffer().is_empty() {
                    reader
                        .get_ref()
                        .set_read_timeout(Some(remaining))
                        .or_else(|error| {
                            // Darwin rejects SO_RCVTIMEO after peer closure while final
                            // response bytes may still be readable. Drain without blocking.
                            if cfg!(target_os = "macos")
                                && error.kind() == std::io::ErrorKind::InvalidInput
                            {
                                reader.get_ref().set_nonblocking(true)
                            } else {
                                Err(error)
                            }
                        })
                        .map_err(|error| transport_error(method, Some(error.kind())))?;
                }
                let available = reader
                    .fill_buf()
                    .map_err(|error| transport_error(method, Some(error.kind())))?;
                if available.is_empty() {
                    return Err(BridgeError::new(ErrorCode::RuntimeUnavailable,
                        "Orca closed the RPC connection before replying; request outcome is unknown", Outcome::Unknown).into());
                }
                let len = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |n| n + 1);
                if line.len().saturating_add(len) > MAX_FRAME {
                    return Err(BridgeError::new(
                        ErrorCode::OutputLimit,
                        "Orca RPC frame exceeds 32 MiB; request outcome is unknown",
                        Outcome::Unknown,
                    )
                    .into());
                }
                let complete = available[len - 1] == b'\n';
                line.extend_from_slice(&available[..len]);
                reader.consume(len);
                if complete {
                    break;
                }
            }
            let response: Value = serde_json::from_slice(&line).map_err(|_| {
                BridgeError::new(
                    ErrorCode::InvalidResponse,
                    "Invalid Orca RPC JSON; request outcome is unknown",
                    Outcome::Unknown,
                )
            })?;
            if response.get("_keepalive").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if response.get("id").and_then(Value::as_str) != Some(&id) {
                return Err(BridgeError::new(
                    ErrorCode::InvalidResponse,
                    "Orca RPC response ID mismatch; request outcome is unknown",
                    Outcome::Unknown,
                )
                .into());
            }
            if let Some(runtime_id) = response.pointer("/_meta/runtimeId").and_then(Value::as_str)
                && runtime_id != metadata.runtime_id
            {
                return Err(BridgeError::new(
                    ErrorCode::RuntimeUnavailable,
                    "Orca restarted during the request; reacquire handles",
                    Outcome::Unknown,
                )
                .into());
            }
            match response.get("ok").and_then(Value::as_bool) {
                Some(true) => return Ok(response.get("result").cloned().unwrap_or(Value::Null)),
                Some(false) => {
                    let native = &response["error"];
                    let code = native["code"].as_str().unwrap_or("rpc_error");
                    let message = native["message"]
                        .as_str()
                        .unwrap_or("Unknown runtime error");
                    return Err(BridgeError::new(
                        ErrorCode::NativeError,
                        format!(
                            "Orca {}: {}: {}",
                            diagnostic(method),
                            diagnostic(code),
                            diagnostic(message)
                        ),
                        Outcome::Unknown,
                    )
                    .with_native(&response)
                    .into());
                }
                None => {
                    return Err(BridgeError::new(
                        ErrorCode::InvalidResponse,
                        "Orca RPC response omitted a valid success status",
                        Outcome::Unknown,
                    )
                    .into());
                }
            }
        }
    }
}

fn timeout_error(method: &str) -> BridgeError {
    BridgeError::new(
        ErrorCode::RpcTimeout,
        format!("Orca RPC timed out: {}", diagnostic(method)),
        Outcome::Unknown,
    )
}

fn transport_error(method: &str, kind: Option<std::io::ErrorKind>) -> BridgeError {
    match kind {
        Some(std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
            timeout_error(method)
        }
        Some(std::io::ErrorKind::Interrupted) => BridgeError::new(
            ErrorCode::Interrupted,
            "Orca RPC interrupted; request outcome is unknown",
            Outcome::Unknown,
        ),
        _ => BridgeError::new(
            ErrorCode::RuntimeUnavailable,
            format!(
                "Reading Orca RPC response failed: {}; request outcome is unknown",
                diagnostic(method)
            ),
            Outcome::Unknown,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    fn fixture(
        response: impl FnOnce(Value) -> Value + Send + 'static,
    ) -> (tempfile::TempDir, RpcClient, std::thread::JoinHandle<()>) {
        raw_fixture(Duration::from_secs(2), move |socket, request| {
            let reply = response(request);
            writeln!(socket, "{{\"_keepalive\":true}}").unwrap();
            writeln!(socket, "{reply}").unwrap();
        })
    }

    fn raw_fixture(
        timeout: Duration,
        response: impl FnOnce(&mut UnixStream, Value) + Send + 'static,
    ) -> (tempfile::TempDir, RpcClient, std::thread::JoinHandle<()>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::write(dir.path().join("orca-runtime.json"), json!({"runtimeId":"fixture","authToken":"fixture-only-token","transports":[{"kind":"unix","endpoint":path}]}).to_string()).unwrap();
        let client = RpcClient {
            metadata_path: dir.path().join("orca-runtime.json"),
            timeout,
        };
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["authToken"], "fixture-only-token");
            response(&mut socket, request);
        });
        (dir, client, server)
    }
    #[test]
    fn authenticates_and_ignores_keepalives() {
        let (_dir, c, s) = fixture(
            |r| json!({"id":r["id"],"ok":true,"result":{"answer":42},"_meta":{"runtimeId":"fixture"}}),
        );
        assert_eq!(c.call("status.get", json!({})).unwrap()["answer"], 42);
        s.join().unwrap();
    }

    #[test]
    fn partial_frame_cannot_extend_response_deadline() {
        let (_dir, c, s) = raw_fixture(Duration::from_millis(100), |socket, request| {
            for _ in 0..12 {
                if socket.write_all(b" ").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = writeln!(
                socket,
                "{}",
                json!({"id":request["id"],"ok":true,"result":{}})
            );
        });
        assert!(
            c.call("status.get", json!({}))
                .unwrap_err()
                .to_string()
                .contains("Orca RPC timed out: status.get")
        );
        s.join().unwrap();
    }

    #[test]
    fn keepalive_does_not_reset_response_deadline() {
        let (_dir, c, s) = raw_fixture(Duration::from_millis(150), |socket, request| {
            std::thread::sleep(Duration::from_millis(100));
            if writeln!(socket, "{{\"_keepalive\":true}}").is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
            let _ = writeln!(
                socket,
                "{}",
                json!({"id":request["id"],"ok":true,"result":{}})
            );
        });
        assert!(
            c.call("status.get", json!({}))
                .unwrap_err()
                .to_string()
                .contains("Orca RPC timed out: status.get")
        );
        s.join().unwrap();
    }
    #[test]
    fn rejects_runtime_change() {
        let (_dir, c, s) = fixture(
            |r| json!({"id":r["id"],"ok":true,"result":{},"_meta":{"runtimeId":"restarted"}}),
        );
        assert!(
            c.call("status.get", json!({}))
                .unwrap_err()
                .to_string()
                .contains("restarted")
        );
        s.join().unwrap();
    }

    #[test]
    fn native_errors_preserve_codes_and_retry_ids_without_raw_details() {
        let (_dir, client, server) = fixture(
            |request| json!({"id":request["id"],"ok":false,"error":{"code":"fixture_rejected","message":"Fixture rejected","outcome":"failed","details":{"operationId":"operation-fixture","retryRequestId":"request-fixture","authToken":"private fixture token","params":{"prompt":"private prompt fixture"}}}}),
        );
        let error = client.call("fixture.read", json!({})).unwrap_err();
        let envelope = crate::errors::envelope(&error);
        assert_eq!(envelope["error"]["code"], "native_error");
        assert_eq!(envelope["error"]["nativeCode"], "fixture_rejected");
        assert_eq!(envelope["error"]["operationId"], "operation-fixture");
        assert_eq!(envelope["error"]["retryRequestId"], "request-fixture");
        assert_eq!(envelope["error"]["outcome"], "failed");
        assert!(!envelope.to_string().contains("private"));
        server.join().unwrap();
    }

    #[test]
    fn metadata_failure_is_not_started_and_response_mismatch_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let client = RpcClient {
            metadata_path: dir.path().join("missing"),
            timeout: Duration::from_secs(1),
        };
        let error = client.call("fixture.read", json!({})).unwrap_err();
        let envelope = crate::errors::envelope(&error);
        assert_eq!(envelope["error"]["code"], "runtime_unavailable");
        assert_eq!(envelope["error"]["outcome"], "not-started");
        let (_dir, client, server) =
            fixture(|_| json!({"id":"different-fixture","ok":true,"result":{}}));
        let error = client.call("fixture.read", json!({})).unwrap_err();
        let envelope = crate::errors::envelope(&error);
        assert_eq!(envelope["error"]["code"], "invalid_response");
        assert_eq!(envelope["error"]["outcome"], "unknown");
        server.join().unwrap();
    }
}
