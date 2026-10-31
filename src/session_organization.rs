//! Orca owns transfer receipts and naming plans; this client never writes Orca state.
use crate::{config::Config, history, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub const TAB_TRANSFER_CAPABILITY: &str = "terminal.tab-transfer.v1";
pub const SESSION_NAMES_CAPABILITY: &str = "session.names.v1";

fn require_capability(client: &RpcClient, capability: &str) -> Result<()> {
    let status = client.call("status.get", Value::Null)?;
    ensure!(
        status["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == capability)),
        "This Orca execution host does not advertise {capability}; load a compatible native build. No mutation was requested"
    );
    Ok(())
}

pub fn transfer_tab(
    config: &Config,
    terminal: &str,
    destination: &str,
    request_id: &str,
) -> Result<Value> {
    ensure!(
        !destination.trim().is_empty(),
        "Missing destination workspace"
    );
    validate_receipt_id(request_id)?;
    let client = RpcClient::new(config);
    require_capability(&client, TAB_TRANSFER_CAPABILITY)?;
    let shown = client.call("terminal.show", json!({"terminal": terminal}))?;
    let source = shown.get("terminal").context("Missing source terminal")?;
    let incarnation = source["incarnationId"]
        .as_str()
        .context("Source terminal incarnation is unknown")?;
    let workspace = source["worktreeId"]
        .as_str()
        .context("Source terminal workspace is unknown")?;
    client
        .call(
            "terminal.transferTab",
            json!({
                "terminal": terminal,
                "destination": destination,
                "expectedIncarnationId": incarnation,
                "expectedWorktreeId": workspace,
                "requestId": request_id
            }),
        )
        .with_context(|| {
            format!(
                "Tab transfer request {request_id} was not confirmed; inspect native state and retry with the same --request-id"
            )
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NameSelection {
    pub session: String,
    pub title: String,
    #[serde(default)]
    pub replace_manual: bool,
}

pub fn preview_names(config: &Config, selections: Vec<NameSelection>) -> Result<Value> {
    ensure!(
        !selections.is_empty() && selections.len() <= 100,
        "Select between 1 and 100 sessions explicitly"
    );
    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(selections.len());
    let sessions = history::load_sessions(config)?;
    for selection in selections {
        ensure!(
            !selection.title.trim().is_empty()
                && selection.title.len() <= 512
                && !selection.title.chars().any(char::is_control),
            "Each task title must be nonempty, at most 512 bytes, and contain no control characters"
        );
        let session = history::resolve_session(&sessions, &selection.session)?;
        ensure!(
            seen.insert(session.id),
            "A session was selected more than once"
        );
        items.push(json!({
            "session": {
                "agent": session.agent,
                "sessionId": session.session_id,
                "executionHostId": session.host
            },
            "title": selection.title,
            "replaceManual": selection.replace_manual
        }));
    }
    let client = RpcClient::new(config);
    require_capability(&client, SESSION_NAMES_CAPABILITY)?;
    client.call("session.names.preview", json!({"selection": items}))
}

pub fn apply_names(config: &Config, plan_id: &str) -> Result<Value> {
    names_receipt(config, "session.names.apply", "planId", plan_id)
}

pub fn undo_names(config: &Config, change_id: &str) -> Result<Value> {
    names_receipt(config, "session.names.undo", "changeId", change_id)
}

fn names_receipt(config: &Config, method: &str, field: &str, id: &str) -> Result<Value> {
    validate_receipt_id(id)?;
    let client = RpcClient::new(config);
    require_capability(&client, SESSION_NAMES_CAPABILITY)?;
    client.call(method, json!({field: id}))
}

fn validate_receipt_id(id: &str) -> Result<()> {
    ensure!(
        !id.trim().is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
        "Native receipt ID must be nonempty, at most 128 bytes, and contain no control characters"
    );
    Ok(())
}
