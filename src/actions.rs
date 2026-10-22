use crate::{config::Config, identity, model::Session, rpc::RpcClient};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Command};
use url::Url;

/// Delegate every argument literally; Orca owns command validation and mutation recovery.
pub fn orca(config: &Config, args: &[String]) -> Result<Value> {
    let mut command = Command::new(&config.orca_cli);
    command
        .args(args)
        .env("ORCA_USER_DATA_PATH", &config.data_dir);
    if !args.iter().any(|arg| arg == "--json") {
        command.arg("--json");
    }
    let output = command
        .output()
        .with_context(|| format!("Cannot execute {}", config.orca_cli.display()))?;
    let value: Value =
        serde_json::from_slice(&output.stdout).context("Orca CLI did not return JSON")?;
    if !output.status.success() || value["ok"] == false {
        // CLI errors may include a recovery command; retain it for a deliberate retry.
        bail!(
            "Orca command failed: {}",
            value.get("error").unwrap_or(&value)
        );
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

pub fn open_session(config: &Config, session: &Session) -> Result<Value> {
    let resolution = identity::resolve(config, session)?;
    ensure!(
        resolution.matches.len() <= 1,
        "This conversation has multiple proven live terminals; select a terminal explicitly"
    );
    if let Some(live) = resolution.matches.first() {
        let focus = RpcClient::new(config).call("terminal.focus", json!({"terminal":live.terminal,"expectedIncarnationId":live.incarnation_id,"navigation":"host"}))?;
        return Ok(json!({"action":"focused","identity":live,"result":focus}));
    }
    ensure!(
        resolution.unresolved_terminals.is_empty(),
        "Cannot prove this history session is closed: {} terminal identities remain unknown. Select a live terminal explicitly, or run `sessions resume` to request a historical resume",
        resolution.unresolved_terminals.len()
    );
    resume_session(config, session)
}

pub fn stop_terminal(config: &Config, handle: &str, incarnation: Option<&str>) -> Result<Value> {
    let rpc = RpcClient::new(config);
    let value = rpc.call("terminal.show", json!({"terminal":handle}))?;
    let terminal = value.get("terminal").context("Missing terminal")?;
    ensure!(
        incarnation.is_none_or(|i| terminal["incarnationId"] == i),
        "Terminal incarnation changed; refresh before stopping"
    );
    let pty = terminal["ptyId"]
        .as_str()
        .context("Terminal has no live PTY")?;
    let worktree = terminal["worktreeId"]
        .as_str()
        .context("Terminal has no worktree")?;
    // This operation atomically targets exact PTY IDs, unlike legacy handle closes.
    let result = rpc.call(
        "terminal.stopExact",
        json!({"worktree":worktree,"expectedPtyIds":[pty],"targetOnly":true,"keepHistory":true}),
    )?;
    ensure!(
        result["postStopVerified"] == true
            && result["stoppedPtyIds"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == pty)),
        "Orca did not verify the requested PTY stopped; refresh terminal state before retrying"
    );
    Ok(result)
}

pub fn rename_terminal(config: &Config, handle: &str, title: &str) -> Result<Value> {
    let rpc = RpcClient::new(config);
    let result = rpc.call("terminal.rename", json!({"terminal":handle,"title":title}))?;
    let observed = rpc.call("terminal.show", json!({"terminal":handle})).ok();
    let observed_title = observed
        .as_ref()
        .and_then(|v| v.pointer("/terminal/title"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(
        json!({"acknowledged":true,"verified":observed_title.as_str()==Some(title),"observedTitle":observed_title,"result":result}),
    )
}

pub fn stop_session(config: &Config, session: &Session) -> Result<Value> {
    let live = single_live(config, session)?;
    stop_terminal(config, &live.terminal, Some(&live.incarnation_id))
}

fn single_live(config: &Config, session: &Session) -> Result<identity::LiveIdentity> {
    let mut resolution = identity::resolve(config, session)?;
    ensure!(
        resolution.matches.len() == 1,
        "Expected exactly one proven live terminal for this session; choose a terminal handle explicitly"
    );
    Ok(resolution.matches.remove(0))
}

/// Explicit historical resume. Orca's preparation rejects its known durable ownership conflicts.
pub fn resume_session(config: &Config, session: &Session) -> Result<Value> {
    ensure!(
        session.host == "local",
        "Historical resume currently supports only local sessions"
    );
    let file = session
        .file_path
        .as_ref()
        .context("Session has no transcript path")?;
    let cwd = session
        .cwd
        .as_ref()
        .context("Session has no working directory; select a workspace explicitly")?;
    ensure!(
        std::path::Path::new(cwd).is_dir(),
        "Session working directory is unavailable"
    );
    let argv = resume_argv(&session.agent, &session.session_id, file)?;
    let client = RpcClient::new(config);
    let codex_home = session.codex_home.as_deref();
    let preparation = client.call("aiVault.prepareSessionResume", json!({"agent":session.agent,"sessionId":session.session_id,"filePath":file,"codexHome":codex_home,"executionHostId":"local"}))?;
    let mut env = BTreeMap::new();
    let mut env_to_delete = Vec::new();
    if session.agent == "codex" {
        if preparation["useRealCodexHome"] == true || codex_home.is_none() {
            env_to_delete.extend(["CODEX_HOME", "ORCA_CODEX_HOME"]);
        } else if let Some(home) = preparation["substituteCodexHome"].as_str().or(codex_home) {
            env.insert("CODEX_HOME", home);
        }
    }
    // terminal.create's API accepts one shell command. Generate it only from a known argv,
    // quote every argument, and invoke /bin/sh explicitly so the user's shell cannot change grammar.
    let inner = format!(
        "cd {} && exec {}",
        quote_posix(cwd),
        argv.iter()
            .map(|a| quote_posix(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let command = format!("/bin/sh -c {}", quote_posix(&inner));
    let key = if matches!(session.agent.as_str(), "cursor" | "antigravity") {
        "conversation_id"
    } else {
        "session_id"
    };
    let result = client.call(
        "terminal.create",
        json!({
            "worktree":format!("path:{cwd}"),"clientMutationId":uuid::Uuid::new_v4().to_string(),
            "command":command,"env":env,"envToDelete":env_to_delete,
            "resumeProviderSession":{"key":key,"id":session.session_id,"transcriptPath":file},
            "launchAgent":session.agent,"title":session.title,"focus":true,"presentation":"focused",
            "startupCommandDelivery":"shell-ready"
        }),
    )?;
    Ok(json!({"action":"resumed","sessionId":session.id,"result":result}))
}

pub fn link(session: &Session) -> Result<String> {
    session_link(&session.id)
}
fn session_link(id: &str) -> Result<String> {
    let mut url = Url::parse("orca-bridge://session/")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Invalid link base"))?
        .pop_if_empty()
        .push(id);
    Ok(url.into())
}
pub fn parse_link(link: &str) -> Result<String> {
    let url = Url::parse(link).context("Invalid Orca Bridge URL")?;
    ensure!(
        url.scheme() == "orca-bridge"
            && url.host_str() == Some("session")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Expected orca-bridge://session/<encoded-session-id>"
    );
    let segments = url
        .path_segments()
        .context("Missing session ID")?
        .collect::<Vec<_>>();
    ensure!(
        segments.len() == 1 && !segments[0].is_empty(),
        "Expected one encoded session ID"
    );
    // URL path decoding must happen exactly once; durable IDs can themselves contain percent escapes.
    let decoded = percent_decode(segments[0])?;
    ensure!(
        !decoded.chars().any(char::is_control),
        "Session ID contains a control character"
    );
    Ok(decoded)
}
fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            ensure!(index + 2 < bytes.len(), "Invalid percent escape");
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])?;
            result.push(u8::from_str_radix(hex, 16).context("Invalid percent escape")?);
            index += 3;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(result).context("Session ID is not UTF-8")
}
fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn resume_argv(agent: &str, id: &str, file: &str) -> Result<Vec<String>> {
    ensure!(
        !id.is_empty()
            && id.len() <= 512
            && !id.starts_with('-')
            && !id.chars().any(char::is_control),
        "Unsafe provider session ID"
    );
    let values: Vec<&str> = match agent {
        "codex" => vec!["codex", "resume", id],
        "claude" => vec!["claude", "--resume", id],
        "cursor" => vec!["cursor-agent", "--resume", id],
        "gemini" => vec!["gemini", "--resume", id],
        "opencode" => vec!["opencode", "--session", id],
        "opencode2" => vec!["opencode2", "--session", id],
        "pi" => vec!["pi", "--session", file],
        "omp" => vec!["omp", "--resume", file],
        "prime-agent" => vec!["prime-agent", "--resume", file],
        "kimi" => vec!["kimi", "--session", id],
        "muse" => vec!["muse", "resume", id],
        "kiro" => vec!["kiro-cli", "--resume-id", id],
        "droid" => vec!["droid", "--resume", id],
        "grok" => vec!["grok", "--resume", id],
        "devin" => vec!["devin", "--resume", id],
        "jcode" => vec!["jcode", "--resume", id],
        "antigravity" => vec!["agy", "--conversation", id],
        _ => bail!("No verified resume argv for agent {agent}; use the Orca command passthrough"),
    };
    Ok(values.into_iter().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    #[test]
    fn resume_preserves_provider_identity_and_account_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::write(dir.path().join("orca-runtime.json"), json!({"runtimeId":"fixture","authToken":"fixture-only","transports":[{"kind":"unix","endpoint":socket_path}]}).to_string()).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = if index == 0 {
                    json!({"useRealCodexHome":false,"substituteCodexHome":"/fixture/selected-account"})
                } else {
                    json!({"terminal":{"handle":"term_fixture"}})
                };
                writeln!(socket,"{}",json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
                requests.push(request);
            }
            requests
        });
        let config = Config {
            data_dir: dir.path().to_owned(),
            state_dir: dir.path().join("state"),
            orca_cli: dir.path().join("unused"),
            profile_db_override: None,
        };
        let session: Session = serde_json::from_value(json!({"id":"local:codex:fixture","host":"local","agent":"codex","sessionId":"fixture","title":"Fixture","cwd":dir.path(),"branch":null,"model":null,"createdAt":null,"updatedAt":null,"messageCount":0,"filePath":"/fixture/transcript.jsonl","codexHome":"/fixture/original-account","totalTokens":null,"usage":null,"score":null})).unwrap();
        assert_eq!(
            resume_session(&config, &session).unwrap()["action"],
            "resumed"
        );
        let requests = server.join().unwrap();
        assert_eq!(requests[0]["method"], "aiVault.prepareSessionResume");
        assert_eq!(
            requests[0]["params"]["codexHome"],
            "/fixture/original-account"
        );
        let params = &requests[1]["params"];
        assert_eq!(requests[1]["method"], "terminal.create");
        assert_eq!(params["env"]["CODEX_HOME"], "/fixture/selected-account");
        assert_eq!(params["resumeProviderSession"]["id"], "fixture");
        assert_eq!(
            params["resumeProviderSession"]["transcriptPath"],
            "/fixture/transcript.jsonl"
        );
        assert!(uuid::Uuid::parse_str(params["clientMutationId"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn delegates_arguments_literally_and_selects_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("orca fixture");
        let args_path = dir.path().join("args");
        let data_path = dir.path().join("data-path");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > {}\nprintf '%s' \"$ORCA_USER_DATA_PATH\" > {}\nprintf '%s\\n' '{{\"ok\":true,\"result\":{{\"accepted\":true}}}}'\n",
            quote_posix(args_path.to_str().unwrap()),
            quote_posix(data_path.to_str().unwrap())
        );
        std::fs::write(&executable, body).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = Config {
            data_dir: dir.path().join("chosen data"),
            state_dir: dir.path().join("state"),
            orca_cli: executable,
            profile_db_override: None,
        };
        let dangerous = "literal $(touch should-never-execute); `false`\nsecond line";
        assert_eq!(
            orca(
                &config,
                &[
                    "terminal".into(),
                    "send".into(),
                    "--text".into(),
                    dangerous.into()
                ]
            )
            .unwrap()["accepted"],
            true
        );
        let args = std::fs::read(&args_path).unwrap();
        let args = args
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8(part.to_vec()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["terminal", "send", "--text", dangerous, "--json"]
        );
        assert_eq!(
            std::fs::read_to_string(data_path).unwrap(),
            config.data_dir.to_str().unwrap()
        );
    }
    #[test]
    fn links_roundtrip_exactly_once() {
        let id = "local:codex:hello/%2F?🦀#space here";
        assert_eq!(parse_link(&session_link(id).unwrap()).unwrap(), id);
    }
    #[test]
    fn rejects_other_link_actions() {
        for bad in [
            "orca://session/a",
            "orca-bridge://session/a/b",
            "orca-bridge://session/a?command=delete",
            "orca-bridge://session/%00",
        ] {
            assert!(parse_link(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn quotes_shell_metacharacters_as_data() {
        assert_eq!(quote_posix("foo'$(danger)`x`"), "'foo'\\''$(danger)`x`'");
        assert!(resume_argv("codex", "--help", "/tmp/test").is_err());
        assert_eq!(
            resume_argv("codex", "abc", "/tmp/test").unwrap(),
            vec!["codex", "resume", "abc"]
        );
    }
}
