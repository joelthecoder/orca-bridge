//! Orca 1.4.225 paired WebSocket transport. Secrets never implement Debug.
//! Wire format follows shipped shared/e2ee-crypto.js and paired-e2ee-client-handshake.js.
use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use crypto_box::{
    PublicKey, SalsaBox, SecretKey,
    aead::{Aead, AeadCore, OsRng},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Only loss of a transport is retryable. Auth, schema and consumer errors are terminal.
#[derive(Debug)]
struct TransportError(&'static str);
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for TransportError {}

pub fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<TransportError>().is_some()
}

fn websocket_error(error: tungstenite::Error) -> anyhow::Error {
    match error {
        tungstenite::Error::Io(_)
        | tungstenite::Error::ConnectionClosed
        | tungstenite::Error::AlreadyClosed
        | tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => TransportError("Orca WebSocket transport disconnected").into(),
        _ => anyhow::anyhow!("Orca WebSocket protocol, TLS or handshake was rejected"),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub v: u32,
    pub endpoint: String,
    pub device_token: String,
    pub public_key_b64: String,
    #[serde(default)]
    pub paired_device_id: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl Pairing {
    pub fn parse(input: &str) -> Result<Self> {
        ensure!(
            input.len() <= 200_000,
            "Pairing input exceeds the supported size"
        );
        let text = input.trim();
        let bytes = if text.starts_with('{') {
            text.as_bytes().to_vec()
        } else {
            let code = if text.starts_with("orca://") {
                let url = url::Url::parse(text).context("Invalid Orca pairing URL")?;
                ensure!(
                    url.host_str() == Some("pair") && matches!(url.path(), "" | "/"),
                    "Invalid Orca pairing route"
                );
                url.query_pairs()
                    .find(|(k, _)| k == "code")
                    .map(|(_, v)| v.into_owned())
                    .or_else(|| url.fragment().map(str::to_owned))
                    .context("Pairing URL has no code")?
            } else {
                text.to_owned()
            };
            let code = code
                .trim_end_matches('=')
                .replace('+', "-")
                .replace('/', "_");
            URL_SAFE_NO_PAD
                .decode(code)
                .context("Invalid pairing code encoding")?
        };
        let pairing: Self = serde_json::from_slice(&bytes).context("Invalid pairing credential")?;
        pairing.validate()?;
        Ok(pairing)
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.v == 2, "Unsupported Orca pairing version");
        ensure!(
            self.scope.as_deref().is_none_or(|s| s == "runtime"),
            "Use an Orca runtime pairing code, rather than mobile pairing"
        );
        ensure!(
            !self.device_token.is_empty() && self.device_token.len() <= 65_536,
            "Invalid pairing token"
        );
        let url = url::Url::parse(&self.endpoint).context("Invalid pairing endpoint")?;
        ensure!(
            matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some(),
            "Pairing endpoint must use ws or wss"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Pairing endpoint must not contain credentials or query parameters"
        );
        ensure!(
            STANDARD.decode(&self.public_key_b64).map(|x| x.len()).ok() == Some(32),
            "Invalid Orca public key"
        );
        Ok(())
    }
}

fn credential_path(config: &Config) -> PathBuf {
    config.state_dir.join("pairing.json")
}

pub fn load(config: &Config) -> Result<Pairing> {
    let path = credential_path(config);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(&path).context(
            "No private pairing credential; run `orca-bridge stream pair --stdin` first",
        )?;
        ensure!(
            meta.is_file() && meta.mode() & 0o077 == 0,
            "Pairing credential must be a regular file readable only by its owner"
        );
    }
    Pairing::parse(&fs::read_to_string(path).context("Cannot read private pairing credential")?)
}

/// Validates with Orca before persisting, without returning the secret.
pub fn import_pairing(config: &Config, input: &str) -> Result<Value> {
    let pairing = Pairing::parse(input)?;
    let mut channel = Channel::connect(&pairing)?;
    let result = channel.request_one("status.get", Value::Null);
    channel.close();
    result.context("Orca did not accept this runtime pairing credential")?;
    fs::create_dir_all(&config.state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.state_dir, fs::Permissions::from_mode(0o700))?;
    }
    let temp = config
        .state_dir
        .join(format!(".pairing-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(&pairing)?)?;
        file.sync_all()?;
        fs::rename(&temp, credential_path(config))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(
        json!({"paired":true,"endpoint":pairing.endpoint,"deviceId":pairing.paired_device_id,"credentialPath":credential_path(config)}),
    )
}

pub fn status(config: &Config) -> Value {
    match load(config) {
        Ok(p) => json!({"paired":true,"endpoint":p.endpoint,"deviceId":p.paired_device_id}),
        Err(_) => json!({"paired":false}),
    }
}

/// One authenticated subscription. The caller can reconnect and reconcile using its own cursor.
/// Every response is delivered, including initial snapshots, terminal errors and binary frames.
pub fn watch(
    config: &Config,
    method: &str,
    params: Value,
    stop: &AtomicBool,
    mut emit: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let pairing = load(config)?;
    let mut channel = Channel::connect(&pairing)?;
    let id = uuid::Uuid::new_v4().to_string();
    let result = (|| -> Result<()> {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        channel.send(json!({"id":id,"method":method,"params":params}))?;
        while !stop.load(Ordering::Relaxed) {
            match channel.receive()? {
                Some(Frame::Json(value)) => {
                    if value.get("_keepalive").is_some() {
                        continue;
                    }
                    ensure!(
                        value.get("id").and_then(Value::as_str) == Some(id.as_str()),
                        "Orca returned a mismatched stream request id"
                    );
                    if value.get("ok") == Some(&Value::Bool(false)) {
                        let code = value
                            .pointer("/error/code")
                            .and_then(Value::as_str)
                            .unwrap_or("runtime_error");
                        bail!("Orca stream rejected the request ({code})");
                    }
                    let complete = value.get("streaming") != Some(&Value::Bool(true));
                    emit(json!({"transport":"websocket","method":method,"frame":value}))?;
                    if complete {
                        return Ok(());
                    }
                }
                Some(Frame::Binary(data)) => emit(
                    json!({"transport":"websocket","method":method,"binaryBase64":STANDARD.encode(data)}),
                )?,
                None => {}
            }
        }
        Ok(())
    })();
    channel.close();
    result
}

enum Frame {
    Json(Value),
    Binary(Vec<u8>),
}
struct Channel {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    cipher: SalsaBox,
}

impl Channel {
    fn connect(pairing: &Pairing) -> Result<Self> {
        pairing.validate()?;
        let host_bytes: [u8; 32] = STANDARD
            .decode(&pairing.public_key_b64)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid public key"))?;
        let secret = SecretKey::generate(&mut OsRng);
        let cipher = SalsaBox::new(&PublicKey::from(host_bytes), &secret);
        // Never include tungstenite's URL-bearing error in output: credentials stay private.
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME))
            .max_frame_size(Some(MAX_FRAME));
        let endpoint = url::Url::parse(&pairing.endpoint)?;
        let host = endpoint.host_str().context("Missing WebSocket host")?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port = endpoint
            .port()
            .unwrap_or(if endpoint.scheme() == "wss" { 443 } else { 80 });
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|_| TransportError("Cannot resolve paired Orca endpoint"))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let tcp = addresses
            .filter_map(|address| {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                (!remaining.is_zero())
                    .then(|| TcpStream::connect_timeout(&address, remaining).ok())
                    .flatten()
            })
            .next()
            .ok_or(TransportError(
                "Cannot connect to paired Orca WebSocket endpoint",
            ))?;
        tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(10)))?;
        let (mut socket, _) =
            tungstenite::client_tls_with_config(pairing.endpoint.as_str(), tcp, Some(config), None)
                .map_err(|error| match error {
                    tungstenite::HandshakeError::Failure(error) => websocket_error(error),
                    tungstenite::HandshakeError::Interrupted(_) => {
                        TransportError("Orca WebSocket handshake interrupted").into()
                    }
                })?;
        set_timeout(socket.get_mut(), Duration::from_secs(10))?;
        socket.send(Message::Text(json!({"type":"e2ee_hello","publicKeyB64":STANDARD.encode(secret.public_key().as_bytes())}).to_string().into()))?;
        let ready = loop {
            match socket.read().map_err(websocket_error)? {
                Message::Text(text) => {
                    break serde_json::from_str::<Value>(&text)
                        .context("Invalid Orca handshake response")?;
                }
                Message::Ping(_) => {
                    socket.flush()?;
                }
                Message::Pong(_) => {}
                _ => bail!("Orca closed before completing encrypted handshake"),
            }
        };
        ensure!(
            ready["type"] == "e2ee_ready" && ready.get("v").is_none(),
            "Unsupported Orca encrypted handshake framing"
        );
        let mut channel = Self { socket, cipher };
        channel.send(json!({"type":"e2ee_auth","deviceToken":pairing.device_token,"clientCapabilities":["agent.launch.v2","agent-session.structured.v1"]}))?;
        let auth = match channel.receive()? {
            Some(Frame::Json(v)) => v,
            _ => bail!("No Orca authentication response"),
        };
        ensure!(
            auth["type"] == "e2ee_authenticated",
            "Orca rejected the pairing credential; create or import a current runtime pairing code"
        );
        set_timeout(channel.socket.get_mut(), Duration::from_millis(500))?;
        Ok(channel)
    }

    fn send(&mut self, value: Value) -> Result<()> {
        let nonce = SalsaBox::generate_nonce(&mut OsRng);
        let payload = serde_json::to_vec(&value)?;
        ensure!(
            payload.len() <= 4 * 1024 * 1024,
            "Orca request exceeds transport size limit"
        );
        let ciphertext = self
            .cipher
            .encrypt(&nonce, payload.as_slice())
            .map_err(|_| anyhow::anyhow!("Cannot encrypt Orca request"))?;
        let mut bundle = nonce.to_vec();
        bundle.extend(ciphertext);
        self.socket
            .send(Message::Text(STANDARD.encode(bundle).into()))
            .map_err(websocket_error)?;
        Ok(())
    }

    fn decrypt(&self, bundle: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            bundle.len() >= 40 && bundle.len() <= MAX_FRAME,
            "Invalid Orca encrypted frame size"
        );
        self.cipher
            .decrypt(crypto_box::Nonce::from_slice(&bundle[..24]), &bundle[24..])
            .map_err(|_| anyhow::anyhow!("Cannot authenticate encrypted Orca frame"))
    }

    fn receive(&mut self) -> Result<Option<Frame>> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    ensure!(
                        text.len() <= MAX_FRAME,
                        "Orca frame exceeds transport size limit"
                    );
                    let plaintext = self.decrypt(
                        &STANDARD
                            .decode(text.as_bytes())
                            .context("Invalid encrypted Orca frame encoding")?,
                    )?;
                    return Ok(Some(Frame::Json(
                        serde_json::from_slice(&plaintext).context("Invalid Orca stream JSON")?,
                    )));
                }
                Ok(Message::Binary(bytes)) => {
                    return Ok(Some(Frame::Binary(self.decrypt(&bytes)?)));
                }
                Ok(Message::Ping(_)) => {
                    self.socket.flush()?;
                }
                Ok(Message::Pong(_)) => {}
                Ok(Message::Close(_)) => {
                    return Err(TransportError("Orca closed the native stream").into());
                }
                Ok(Message::Frame(_)) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(websocket_error(error)),
            }
        }
    }

    fn request_one(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(Frame::Json(v)) = self.receive()? {
                if v.get("_keepalive") == Some(&Value::Bool(true)) {
                    continue;
                }
                ensure!(
                    v["id"] == id && v["ok"] == true,
                    "Pairing verification request failed"
                );
                return Ok(v["result"].clone());
            }
        }
        bail!("Pairing verification timed out")
    }

    fn close(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }
}

fn set_timeout(stream: &mut MaybeTlsStream<TcpStream>, timeout: Duration) -> Result<()> {
    match stream {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(timeout))?,
        MaybeTlsStream::Rustls(s) => s.get_mut().set_read_timeout(Some(timeout))?,
        _ => bail!("Unsupported WebSocket TLS backend"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abrupt_eof_reconnects_but_protocol_rejection_does_not() {
        use tungstenite::{Error, error::ProtocolError};
        assert!(retryable(&websocket_error(Error::Protocol(
            ProtocolError::ResetWithoutClosingHandshake
        ))));
        assert!(!retryable(&websocket_error(Error::Protocol(
            ProtocolError::UnmaskedFrameFromClient
        ))));
    }

    #[test]
    fn pairing_code_parses_without_exposing_credentials() {
        let payload = json!({"v":2,"endpoint":"ws://127.0.0.1:6768","deviceToken":"example-token","publicKeyB64":STANDARD.encode([1u8;32]),"scope":"runtime"});
        let url = format!(
            "orca://pair?code={}",
            URL_SAFE_NO_PAD.encode(payload.to_string())
        );
        let p = Pairing::parse(&url).unwrap();
        assert_eq!(p.endpoint, "ws://127.0.0.1:6768");
        assert!(Pairing::parse(&url.replace("orca://pair?", "orca://pairing?")).is_err());
    }

    #[test]
    fn matches_shipped_tweetnacl_wire_fixture() {
        // Produced by Orca's installed tweetnacl, using synthetic fixed keys only.
        let secret = SecretKey::from(std::array::from_fn(|i| (i + 1) as u8));
        let peer = PublicKey::from([
            88, 105, 175, 244, 80, 84, 151, 50, 203, 170, 237, 94, 93, 249, 179, 10, 109, 163, 28,
            176, 229, 116, 43, 173, 90, 212, 161, 167, 104, 241, 166, 123,
        ]);
        let nonce = crypto_box::Nonce::from([
            65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86,
            87, 88,
        ]);
        let expected = [
            159, 88, 106, 59, 199, 0, 96, 239, 151, 163, 156, 62, 244, 177, 14, 6, 24, 213, 150,
            90, 109, 5, 239, 63, 44, 63, 100, 196, 141, 14, 173, 130, 206, 25, 111, 33, 65, 78,
            231, 226, 176, 235, 116, 188, 189, 59,
        ];
        let cipher = SalsaBox::new(&peer, &secret);
        assert_eq!(
            cipher
                .encrypt(&nonce, b"{\"fixture\":\"orca-nacl-compat\"}".as_slice())
                .unwrap(),
            expected
        );
        assert_eq!(
            cipher.decrypt(&nonce, expected.as_slice()).unwrap(),
            b"{\"fixture\":\"orca-nacl-compat\"}"
        );
    }

    fn mock_receive(socket: &mut WebSocket<TcpStream>, cipher: &SalsaBox) -> Value {
        let text = socket.read().unwrap().into_text().unwrap();
        let bundle = STANDARD.decode(text.as_bytes()).unwrap();
        let plain = cipher
            .decrypt(crypto_box::Nonce::from_slice(&bundle[..24]), &bundle[24..])
            .unwrap();
        serde_json::from_slice(&plain).unwrap()
    }

    fn mock_send(socket: &mut WebSocket<TcpStream>, cipher: &SalsaBox, value: Value) {
        let nonce = SalsaBox::generate_nonce(&mut OsRng);
        let mut bundle = nonce.to_vec();
        bundle.extend(
            cipher
                .encrypt(&nonce, value.to_string().as_bytes())
                .unwrap(),
        );
        socket
            .send(Message::Text(STANDARD.encode(bundle).into()))
            .unwrap();
    }

    fn fixture_subscription(reply: Value, consumer_error: bool, disconnect: bool) -> Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let secret = SecretKey::generate(&mut OsRng);
        let offer = json!({"v":2,"endpoint":format!("ws://{}",listener.local_addr().unwrap()),"deviceToken":"fixture-only","publicKeyB64":STANDARD.encode(secret.public_key().as_bytes()),"scope":"runtime"});
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: dir.path().to_owned(),
            state_dir: dir.path().to_owned(),
            orca_cli: PathBuf::new(),
            profile_db_override: None,
        };
        fs::write(credential_path(&config), offer.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(credential_path(&config), fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut socket = tungstenite::accept(tcp).unwrap();
            let hello: Value =
                serde_json::from_str(socket.read().unwrap().into_text().unwrap().as_str()).unwrap();
            let key: [u8; 32] = STANDARD
                .decode(hello["publicKeyB64"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let cipher = SalsaBox::new(&PublicKey::from(key), &secret);
            socket
                .send(Message::Text(
                    json!({"type":"e2ee_ready"}).to_string().into(),
                ))
                .unwrap();
            let auth = mock_receive(&mut socket, &cipher);
            assert!(
                auth["clientCapabilities"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("agent-session.structured.v1"))
            );
            mock_send(&mut socket, &cipher, json!({"type":"e2ee_authenticated"}));
            let req = mock_receive(&mut socket, &cipher);
            if disconnect {
                socket.close(None).unwrap();
                return;
            }
            let mut reply = reply;
            reply["id"] = req["id"].clone();
            mock_send(&mut socket, &cipher, reply);
            let _ = socket.read();
        });
        let result = watch(
            &config,
            "fixture.subscribe",
            Value::Null,
            &AtomicBool::new(false),
            |_| {
                if consumer_error {
                    anyhow::bail!("consumer failed")
                }
                Ok(())
            },
        );
        server.join().unwrap();
        result
    }

    #[test]
    fn only_transport_loss_is_retryable() {
        let rejected=fixture_subscription(json!({"ok":false,"error":{"code":"method_not_found","message":"untrusted diagnostic"}}),false,false).unwrap_err();
        assert!(!retryable(&rejected));
        assert!(!rejected.to_string().contains("untrusted diagnostic"));
        let consumer =
            fixture_subscription(json!({"ok":true,"streaming":true,"result":{}}), true, false)
                .unwrap_err();
        assert!(!retryable(&consumer));
        let disconnected = fixture_subscription(Value::Null, false, true).unwrap_err();
        assert!(retryable(&disconnected));
    }

    #[test]
    fn final_response_ends_subscription_without_reconnect() {
        fixture_subscription(json!({"ok":true,"result":{"finished":true}}), false, false).unwrap();
    }

    #[test]
    fn native_pair_import_and_followup_events_reconnect_cleanly() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let secret = SecretKey::generate(&mut OsRng);
        let offer = json!({"v":2,"endpoint":endpoint,"deviceToken":"synthetic-test-token","publicKeyB64":STANDARD.encode(secret.public_key().as_bytes()),"scope":"runtime"});
        let server = std::thread::spawn(move || {
            for n in 0..3 {
                let (tcp, _) = listener.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut socket = tungstenite::accept(tcp).unwrap();
                let hello: Value =
                    serde_json::from_str(socket.read().unwrap().into_text().unwrap().as_str())
                        .unwrap();
                assert_eq!(hello["type"], "e2ee_hello");
                let bytes: [u8; 32] = STANDARD
                    .decode(hello["publicKeyB64"].as_str().unwrap())
                    .unwrap()
                    .try_into()
                    .unwrap();
                let cipher = SalsaBox::new(&PublicKey::from(bytes), &secret);
                socket
                    .send(Message::Text(
                        json!({"type":"e2ee_ready"}).to_string().into(),
                    ))
                    .unwrap();
                let auth = mock_receive(&mut socket, &cipher);
                assert_eq!(auth["deviceToken"], "synthetic-test-token");
                mock_send(&mut socket, &cipher, json!({"type":"e2ee_authenticated"}));
                let req = mock_receive(&mut socket, &cipher);
                if n == 0 {
                    assert_eq!(req["method"], "status.get");
                    mock_send(
                        &mut socket,
                        &cipher,
                        json!({"id":req["id"],"ok":true,"result":{"ready":true}}),
                    );
                } else {
                    assert_eq!(req["method"], "agentSession.subscribeStatus");
                    for kind in ["snapshot", "status"] {
                        mock_send(
                            &mut socket,
                            &cipher,
                            json!({"id":req["id"],"ok":true,"streaming":true,"result":{"type":kind,"sequence":n}}),
                        );
                    }
                }
                assert!(matches!(socket.read().unwrap(), Message::Close(_)));
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: dir.path().to_owned(),
            state_dir: dir.path().join("private"),
            orca_cli: PathBuf::new(),
            profile_db_override: None,
        };
        let report = import_pairing(&config, &offer.to_string()).unwrap();
        assert_eq!(report["paired"], true);
        assert!(!report.to_string().contains("synthetic-test-token"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs::metadata(credential_path(&config)).unwrap().mode() & 0o777,
                0o600
            );
        }
        for _ in 0..2 {
            let stop = AtomicBool::new(false);
            let mut types = Vec::new();
            watch(
                &config,
                "agentSession.subscribeStatus",
                Value::Null,
                &stop,
                |frame| {
                    types.push(
                        frame["frame"]["result"]["type"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    );
                    if types.len() == 2 {
                        stop.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(types, ["snapshot", "status"]);
        }
        server.join().unwrap();
    }
}
