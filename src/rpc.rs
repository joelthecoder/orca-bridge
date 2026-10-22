use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
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
        let metadata: Metadata =
            serde_json::from_slice(&std::fs::read(&self.metadata_path).with_context(|| {
                format!(
                    "Orca runtime metadata unavailable at {}",
                    self.metadata_path.display()
                )
            })?)
            .context("Invalid Orca runtime metadata")?;
        let transport = metadata
            .transports
            .iter()
            .chain(metadata.transport.iter())
            .find(|t| t.kind == "unix")
            .context("Orca has no Unix socket transport")?;
        let mut socket = UnixStream::connect(&transport.endpoint)
            .context("Cannot connect to Orca; is it running?")?;
        socket.set_read_timeout(Some(self.timeout))?;
        socket.set_write_timeout(Some(self.timeout))?;
        let id = uuid::Uuid::new_v4().to_string();
        let request =
            json!({"id":id,"authToken":metadata.auth_token,"method":method,"params":params});
        serde_json::to_writer(&mut socket, &request)?;
        socket.write_all(b"\n")?;
        let mut reader = BufReader::new(socket);
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            ensure!(
                std::time::Instant::now() < deadline,
                "Orca RPC timed out: {method}"
            );
            let mut line = Vec::new();
            loop {
                let available = reader.fill_buf().context("Reading Orca RPC response")?;
                ensure!(
                    !available.is_empty(),
                    "Orca closed the RPC connection before replying"
                );
                let len = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |n| n + 1);
                ensure!(
                    line.len() + len <= MAX_FRAME,
                    "Orca RPC frame exceeds 32 MiB"
                );
                let complete = available[len - 1] == b'\n';
                line.extend_from_slice(&available[..len]);
                reader.consume(len);
                if complete {
                    break;
                }
            }
            let response: Value = serde_json::from_slice(&line).context("Invalid Orca RPC JSON")?;
            if response.get("_keepalive").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            ensure!(
                response.get("id").and_then(Value::as_str) == Some(&id),
                "Orca RPC response ID mismatch"
            );
            if let Some(runtime_id) = response.pointer("/_meta/runtimeId").and_then(Value::as_str) {
                ensure!(
                    runtime_id == metadata.runtime_id,
                    "Orca restarted during the request; reacquire handles"
                );
            }
            if response.get("ok").and_then(Value::as_bool) != Some(true) {
                let code = response
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .unwrap_or("rpc_error");
                let message = response
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown runtime error");
                bail!("Orca {method}: {code}: {message}");
            }
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    fn fixture(
        response: impl FnOnce(Value) -> Value + Send + 'static,
    ) -> (tempfile::TempDir, RpcClient, std::thread::JoinHandle<()>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::write(dir.path().join("orca-runtime.json"), json!({"runtimeId":"fixture","authToken":"fixture-only-token","transports":[{"kind":"unix","endpoint":path}]}).to_string()).unwrap();
        let client = RpcClient {
            metadata_path: dir.path().join("orca-runtime.json"),
            timeout: Duration::from_secs(2),
        };
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["authToken"], "fixture-only-token");
            let reply = response(request);
            writeln!(socket, "{{\"_keepalive\":true}}").unwrap();
            writeln!(socket, "{reply}").unwrap();
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
}
