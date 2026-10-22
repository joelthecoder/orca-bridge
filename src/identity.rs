//! Conservative history-to-terminal correlation. Paths and process fences are evidence; titles are not.
use crate::{config::Config, model::Session, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveIdentity {
    pub terminal: String,
    pub incarnation_id: String,
    pub evidence: String,
    pub provider_session_id: Option<String>,
    pub transcript_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub matches: Vec<LiveIdentity>,
    pub unresolved_terminals: Vec<String>,
}

pub fn resolve(config: &Config, session: &Session) -> Result<Resolution> {
    ensure!(
        session.host == "local",
        "History actions currently support only the local execution host"
    );
    let client = RpcClient::new(config);
    let listed = client.call(
        "terminal.list",
        json!({"requireFreshPtyLiveness":true,"includeVisualLayouts":false}),
    )?;
    let terminals = listed["terminals"]
        .as_array()
        .context("Invalid terminal list response")?;
    let tabs = client.call("session.tabs.listAll", json!({})).ok();
    let mut result = Resolution {
        matches: Vec::new(),
        unresolved_terminals: Vec::new(),
    };
    for terminal in terminals {
        if terminal["connected"] == false
            || terminal["executionHostId"]
                .as_str()
                .is_some_and(|h| h != "local")
        {
            continue;
        }
        let agent = terminal["agentIdentity"].as_str();
        if agent.is_some_and(|a| a != session.agent) {
            continue;
        }
        let Some(handle) = terminal["handle"].as_str() else {
            continue;
        };
        let Some(incarnation) = terminal["incarnationId"].as_str() else {
            result.unresolved_terminals.push(handle.into());
            continue;
        };
        let provider = tabs.as_ref().and_then(|snapshots| {
            provider_for_terminal(snapshots, handle, incarnation, &session.agent)
        });
        if let Some(provider) = provider {
            let id = provider["id"].as_str();
            let paths = provider["transcriptPath"]
                .as_str()
                .map(|s| vec![s.to_owned()])
                .unwrap_or_default();
            if id == Some(session.session_id.as_str())
                || paths
                    .iter()
                    .any(|p| same_path(session.file_path.as_deref(), p))
            {
                result.matches.push(LiveIdentity {
                    terminal: handle.into(),
                    incarnation_id: incarnation.into(),
                    evidence: "orca-provider-session".into(),
                    provider_session_id: id.map(str::to_owned),
                    transcript_paths: paths,
                });
            }
            continue;
        }
        match process_transcripts(&client, handle, incarnation, &session.agent) {
            Ok(Some(paths)) => {
                if paths
                    .iter()
                    .any(|p| same_path(session.file_path.as_deref(), p))
                {
                    result.matches.push(LiveIdentity {
                        terminal: handle.into(),
                        incarnation_id: incarnation.into(),
                        evidence: "fenced-process-open-transcript".into(),
                        provider_session_id: None,
                        transcript_paths: paths,
                    });
                }
            }
            // A shell with no running agent cannot own a live conversation.
            Ok(None) if agent.is_none() => {}
            // Orca retains some orphaned listing rows after the actual PTY is gone.
            // This explicit host verdict is stronger than the listing's connected bit.
            Err(error) if error.to_string().ends_with(": terminal_gone") => {}
            _ => result.unresolved_terminals.push(handle.into()),
        }
    }
    Ok(result)
}

fn provider_for_terminal<'a>(
    value: &'a Value,
    handle: &str,
    incarnation: &str,
    agent: &str,
) -> Option<&'a Value> {
    value["snapshots"]
        .as_array()?
        .iter()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .find_map(|tab| {
            if tab["type"] != "terminal"
                || tab["terminal"] != handle
                || tab["incarnationId"] != incarnation
            {
                return None;
            }
            let status = &tab["agentStatus"];
            if status["agentType"].as_str() != Some(agent) {
                return None;
            }
            let provider = &status["providerSession"];
            provider["id"].as_str().map(|_| provider)
        })
}

fn same_path(target: Option<&str>, path: &str) -> bool {
    let Some(target) = target else { return false };
    // Require existing file identities rather than equating arbitrary path spellings.
    match (std::fs::canonicalize(target), std::fs::canonicalize(path)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn process_transcripts(
    client: &RpcClient,
    handle: &str,
    incarnation: &str,
    agent: &str,
) -> Result<Option<Vec<String>>> {
    let params =
        json!({"terminal":handle,"expectedIncarnationId":incarnation,"scanChildProcesses":true});
    let before = client.call("terminal.inspectProcess", params.clone())?;
    let evidence = &before["process"]["foregroundProcessEvidence"];
    if before["process"]["hasChildProcesses"] == false
        && matches!(
            before["process"]["foregroundProcess"].as_str(),
            Some("zsh" | "bash" | "fish" | "sh")
        )
    {
        return Ok(None);
    }
    ensure!(
        evidence["verdict"] == "live" && evidence["ptyIncarnationId"] == incarnation,
        "Unfenced process evidence"
    );
    let process_name = evidence["processName"]
        .as_str()
        .context("Missing process name")?;
    ensure!(
        process_name == agent || (agent == "claude" && process_name.starts_with("claude")),
        "Foreground process is not the session agent"
    );
    let pid = evidence
        .pointer("/fence/process/pid")
        .and_then(Value::as_u64)
        .context("Missing process PID")?;
    let start_time = evidence
        .pointer("/fence/process/startTime")
        .and_then(Value::as_str)
        .context("Missing process start time")?;
    let observed =
        bounded_output(Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "lstart="]))
            .context("Inspect process start time")?;
    ensure!(
        observed.status.success()
            && String::from_utf8_lossy(&observed.stdout).trim() == start_time.trim(),
        "Process identity changed"
    );
    let output = bounded_output(Command::new("/usr/sbin/lsof").args([
        "-b",
        "-nP",
        "-p",
        &pid.to_string(),
        "-Fn",
    ]))
    .context("Read foreground process open files")?;
    ensure!(
        output.status.success() && output.stdout.len() <= 2 * 1024 * 1024,
        "Open file identity unavailable"
    );
    let paths = transcript_paths(&output.stdout, agent);
    // Codex's foreground process also owns open rollouts for its subagents.
    // Each exact file proves ownership by this terminal; focus reveals that owner.
    ensure!(!paths.is_empty(), "No authoritative open transcript");
    let after = client.call("terminal.inspectProcess", params)?;
    let current = &after["process"]["foregroundProcessEvidence"];
    ensure!(
        current["verdict"] == "live"
            && current["ptyIncarnationId"] == incarnation
            && current["fence"] == evidence["fence"],
        "Terminal process changed during identity resolution"
    );
    Ok(Some(paths))
}

fn bounded_output(command: &mut Command) -> Result<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().context("Missing process stdout")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            anyhow::bail!("Process identity probe exceeded two seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Process output reader failed"))??;
    ensure!(
        stdout.len() <= 2 * 1024 * 1024,
        "Process output exceeded two MiB"
    );
    Ok(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn transcript_paths(bytes: &[u8], agent: &str) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter(|path| {
            let p = Path::new(path);
            p.is_absolute()
                && p.extension().is_some_and(|e| e == "jsonl")
                && match agent {
                    "codex" => p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("rollout-")),
                    "claude" => p.components().any(|c| c.as_os_str() == "projects"),
                    _ => false,
                }
        })
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_only_exact_transcript_files() {
        let data = b"p12\nn/tmp/foo.txt\nn/tmp/rollout-one.jsonl\nn/tmp/rollout-one.jsonl\nn/secret/projects/session.jsonl\n";
        assert_eq!(
            transcript_paths(data, "codex"),
            vec!["/tmp/rollout-one.jsonl"]
        );
        assert_eq!(
            transcript_paths(data, "claude"),
            vec!["/secret/projects/session.jsonl"]
        );
    }
    #[test]
    fn rejects_stale_provider_binding() {
        let value = json!({"snapshots":[{"tabs":[{"type":"terminal","terminal":"term_a","incarnationId":"old","agentStatus":{"agentType":"codex","providerSession":{"id":"s"}}}]}]});
        assert!(provider_for_terminal(&value, "term_a", "new", "codex").is_none());
    }
}
