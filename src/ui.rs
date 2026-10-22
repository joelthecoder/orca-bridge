//! Versioned UI data projection. Location links never establish live conversation ownership.
use crate::{config::Config, history, model::Session, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resource<T> {
    pub available: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}
impl<T> Resource<T> {
    fn from_result(result: Result<T>) -> Self {
        match result {
            Ok(data) => Self {
                available: true,
                data: Some(data),
                error: None,
            },
            Err(e) => Self {
                available: false,
                data: None,
                error: Some(format!("{e:#}")),
            },
        }
    }
    fn invalidate(&mut self) {
        self.available = false;
        self.data = None;
        self.error = Some("Runtime changed or could not be fenced during snapshot; refresh".into());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitRemote {
    pub canonical_key: Option<String>,
    pub remote_name: Option<String>,
    /// Credential-free URL. Query/fragment and URL userinfo are removed.
    pub remote_url: Option<String>,
}
impl GitRemote {
    fn redact(&mut self) {
        if let Some(text) = &mut self.remote_url {
            if let Ok(mut url) = url::Url::parse(text) {
                if !matches!(url.scheme(), "http" | "https" | "ssh" | "git")
                    || url.host_str().is_none()
                {
                    self.remote_url = None;
                    return;
                }
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                *text = url.into();
            } else if !text.contains('@') || !text.contains(':') {
                // Only recognized URL or conventional SSH scp forms are returned.
                self.remote_url = None;
            } else {
                let parts: Vec<_> = text.split('@').collect();
                if parts.len() != 2 || parts[0].contains(':') || parts[0].contains('/') {
                    self.remote_url = None;
                }
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub display_name: Option<String>,
    pub kind: Option<String>,
    pub badge_color: Option<String>,
    pub git_remote_identity: Option<GitRemote>,
    #[serde(default)]
    pub source_repo_ids: Vec<String>,
    /// Present only when --icons is requested. Orca-owned image/lucide/emoji shape.
    pub repo_icon: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: String,
    pub display_name: Option<String>,
    pub path: Option<String>,
    pub kind: Option<String>,
    pub git_remote_identity: Option<GitRemote>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub id: String,
    pub identity: Option<Value>,
    pub instance_id: Option<String>,
    pub project_id: Option<String>,
    pub repo_id: Option<String>,
    pub host_id: Option<String>,
    pub path: Option<String>,
    pub display_name: Option<String>,
    pub branch: Option<String>,
    pub is_archived: Option<bool>,
    pub is_unread: Option<bool>,
    pub last_activity_at: Option<Value>,
    pub workspace_status: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Terminal {
    pub handle: String,
    pub incarnation_id: Option<String>,
    pub pty_id: Option<String>,
    pub execution_host_id: Option<String>,
    pub worktree_id: Option<String>,
    pub tab_id: Option<String>,
    pub title: Option<String>,
    pub agent_identity: Option<String>,
    pub connected: Option<bool>,
    pub writable: Option<bool>,
    pub orphaned: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    pub worktree_id: String,
    pub publication_epoch: Option<String>,
    pub snapshot_version: Option<u64>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub terminal: Option<String>,
    pub incarnation_id: Option<String>,
    pub is_active: Option<bool>,
    pub status: Option<Value>,
    /// Orca-owned status object; nullable and version-dependent.
    pub agent_status: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub runtime_id: String,
    pub app_version: Option<String>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LiveBinding {
    pub terminal: String,
    pub incarnation_id: String,
    pub tab_id: String,
    pub runtime_id: String,
    /// Currently orca-provider-session: exact provider ID, agent and incarnation.
    pub evidence: String,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    #[serde(flatten)]
    pub session: Session,
    /// Exact local cwd/worktree path metadata links. These are not ownership evidence.
    pub workspace_matches: Vec<String>,
    pub workspace_contexts: Vec<WorkspaceContext>,
    pub live_bindings: Vec<LiveBinding>,
    /// live only with provider and incarnation proof; absence remains unknown, never closed.
    pub live_state: LiveState,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceContext {
    pub worktree_id: String,
    pub folder: Option<String>,
    pub repository_id: Option<String>,
    pub repository_name: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub git_remote: Option<GitRemote>,
    /// cwd-equals-worktree-path associates location, not a live conversation.
    pub evidence: String,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LiveState {
    Live,
    Unknown,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub runtime: Resource<Runtime>,
    pub sessions: Resource<Vec<SessionView>>,
    pub projects: Resource<Vec<Project>>,
    pub repositories: Resource<Vec<Repository>>,
    pub worktrees: Resource<Vec<Worktree>>,
    pub terminals: Resource<Vec<Terminal>>,
    pub tabs: Resource<Vec<Tab>>,
    pub history_truncated: bool,
    pub worktrees_truncated: Option<bool>,
}

fn rows<T: serde::de::DeserializeOwned>(value: Value, key: &str) -> Result<Vec<T>> {
    serde_json::from_value(
        value
            .get(key)
            .cloned()
            .with_context(|| format!("Missing {key} inventory"))?,
    )
    .with_context(|| format!("Unsupported {key} inventory schema"))
}
fn tabs(value: Value) -> Result<Vec<Tab>> {
    let mut out = Vec::new();
    for snapshot in value["snapshots"]
        .as_array()
        .context("Missing tab snapshots")?
    {
        for tab in snapshot["tabs"]
            .as_array()
            .context("Missing tab inventory")?
        {
            let mut tab = tab.clone();
            tab["worktreeId"] = snapshot["worktree"].clone();
            tab["publicationEpoch"] = snapshot["publicationEpoch"].clone();
            tab["snapshotVersion"] = snapshot["snapshotVersion"].clone();
            out.push(serde_json::from_value(tab).context("Unsupported tab schema")?);
        }
    }
    Ok(out)
}

pub fn snapshot(config: &Config, limit: usize, icons: bool) -> Result<Snapshot> {
    ensure!(
        (1..=2000).contains(&limit),
        "Snapshot limit must be between 1 and 2000"
    );
    let rpc = RpcClient::new(config).with_timeout(Duration::from_secs(3));
    let runtime: Resource<Runtime> = Resource::from_result(
        rpc.call("status.get", Value::Null)
            .and_then(|v| Ok(serde_json::from_value(v)?)),
    );
    let mut history_truncated = false;
    let sessions = Resource::from_result(history::load_sessions(config).map(|all| {
        history_truncated = all.len() > limit;
        history::recent(all, limit)
            .into_iter()
            .map(|session| SessionView {
                session,
                workspace_matches: Vec::new(),
                workspace_contexts: Vec::new(),
                live_bindings: Vec::new(),
                live_state: LiveState::Unknown,
            })
            .collect::<Vec<_>>()
    }));
    let mut projects: Resource<Vec<Project>> = Resource::from_result(
        rpc.call("project.list", Value::Null)
            .and_then(|v| rows(v, "projects")),
    );
    let mut repositories: Resource<Vec<Repository>> = Resource::from_result(
        rpc.call("repo.list", Value::Null)
            .and_then(|v| rows(v, "repos")),
    );
    let worktree_result = rpc.call("worktree.list", json!({}));
    let worktrees_truncated = worktree_result
        .as_ref()
        .ok()
        .and_then(|v| v["truncated"].as_bool());
    let mut worktrees: Resource<Vec<Worktree>> =
        Resource::from_result(worktree_result.and_then(|v| rows(v, "worktrees")));
    let mut terminals: Resource<Vec<Terminal>> = Resource::from_result(
        rpc.call("terminal.list", json!({"includeVisualLayouts":false}))
            .and_then(|v| rows(v, "terminals")),
    );
    let mut tabs =
        Resource::from_result(rpc.call("session.tabs.listAll", json!({})).and_then(tabs));
    let end = rpc.call("status.get", Value::Null);
    let consistent = runtime.data.as_ref().is_some_and(|r| {
        end.as_ref().ok().and_then(|v| v["runtimeId"].as_str()) == Some(r.runtime_id.as_str())
    });
    if !consistent {
        projects.invalidate();
        repositories.invalidate();
        worktrees.invalidate();
        terminals.invalidate();
        tabs.invalidate();
    }
    for project in projects.data.iter_mut().flatten() {
        if !icons {
            project.repo_icon = None;
        }
        if let Some(remote) = &mut project.git_remote_identity {
            remote.redact();
        }
    }
    for repo in repositories.data.iter_mut().flatten() {
        if let Some(remote) = &mut repo.git_remote_identity {
            remote.redact();
        }
    }
    let mut snapshot = Snapshot {
        schema_version: 1,
        captured_at: chrono::Utc::now().to_rfc3339(),
        runtime,
        sessions,
        projects,
        repositories,
        worktrees,
        terminals,
        tabs,
        history_truncated,
        worktrees_truncated,
    };
    link_sessions(&mut snapshot);
    Ok(snapshot)
}

fn link_sessions(snapshot: &mut Snapshot) {
    for view in snapshot.sessions.data.iter_mut().flatten() {
        for tree in snapshot.worktrees.data.iter().flatten() {
            if tree.host_id.as_deref() == Some("local")
                && tree.path.is_some()
                && tree.path == view.session.cwd
            {
                view.workspace_matches.push(tree.id.clone());
                let repo = snapshot
                    .repositories
                    .data
                    .iter()
                    .flatten()
                    .find(|r| Some(&r.id) == tree.repo_id.as_ref());
                let project = snapshot
                    .projects
                    .data
                    .iter()
                    .flatten()
                    .find(|p| Some(&p.id) == tree.project_id.as_ref());
                view.workspace_contexts.push(WorkspaceContext {
                    worktree_id: tree.id.clone(),
                    folder: tree.path.clone(),
                    repository_id: tree.repo_id.clone(),
                    repository_name: repo.and_then(|r| r.display_name.clone()),
                    project_id: tree.project_id.clone(),
                    project_name: project.and_then(|p| p.display_name.clone()),
                    git_remote: repo.and_then(|r| r.git_remote_identity.clone()),
                    evidence: "cwd-equals-worktree-path".into(),
                });
            }
        }
        for tab in snapshot.tabs.data.iter().flatten() {
            let Some(terminal) = snapshot.terminals.data.iter().flatten().find(|t| {
                Some(&t.handle) == tab.terminal.as_ref()
                    && t.incarnation_id.is_some()
                    && t.incarnation_id == tab.incarnation_id
                    && t.connected == Some(true)
                    && t.execution_host_id.as_deref() == Some("local")
            }) else {
                continue;
            };
            let Some(status) = &tab.agent_status else {
                continue;
            };
            if status["agentType"].as_str() != Some(view.session.agent.as_str())
                || status
                    .pointer("/providerSession/id")
                    .and_then(Value::as_str)
                    != Some(view.session.session_id.as_str())
            {
                continue;
            }
            let Some(runtime) = &snapshot.runtime.data else {
                continue;
            };
            view.live_bindings.push(LiveBinding {
                terminal: terminal.handle.clone(),
                incarnation_id: terminal.incarnation_id.clone().unwrap(),
                tab_id: tab.id.clone(),
                runtime_id: runtime.runtime_id.clone(),
                evidence: "orca-provider-session".into(),
            });
        }
        if !view.live_bindings.is_empty() {
            view.live_state = LiveState::Live;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Snapshot {
        let session:Session=serde_json::from_value(json!({"id":"local:codex:provider","host":"local","agent":"codex","sessionId":"provider","title":"Same title","cwd":"/fixture/repo","messageCount":1})).unwrap();
        Snapshot {schema_version:1,captured_at:"fixture".into(),runtime:Resource::from_result(Ok(Runtime {runtime_id:"runtime".into(),app_version:None})),
            sessions:Resource::from_result(Ok(vec![SessionView {session,workspace_matches:vec![],workspace_contexts:vec![],live_bindings:vec![],live_state:LiveState::Unknown}])),
            projects:Resource::from_result(Ok(vec![serde_json::from_value(json!({"id":"project","displayName":"A project"})).unwrap()])),
            repositories:Resource::from_result(Ok(vec![serde_json::from_value(json!({"id":"repo","displayName":"A repo","path":"/fixture/repo"})).unwrap()])),
            worktrees:Resource::from_result(Ok(vec![serde_json::from_value(json!({"id":"tree","identity":{"repoId":"repo","path":"/fixture/repo"},"projectId":"project","repoId":"repo","hostId":"local","path":"/fixture/repo"})).unwrap()])),
            terminals:Resource::from_result(Ok(vec![serde_json::from_value(json!({"handle":"term","incarnationId":"inc","connected":true,"executionHostId":"local"})).unwrap()])),
            tabs:Resource::from_result(Ok(vec![serde_json::from_value(json!({"id":"tab","worktreeId":"tree","terminal":"term","incarnationId":"inc","title":"Same title","agentStatus":{"agentType":"codex","providerSession":{"id":"other-provider"}}})).unwrap()])),
            history_truncated:false,worktrees_truncated:Some(false)}
    }
    #[test]
    fn location_links_do_not_establish_live_ownership() {
        let mut snapshot = sample();
        link_sessions(&mut snapshot);
        let view = &snapshot.sessions.data.as_ref().unwrap()[0];
        assert_eq!(
            view.workspace_contexts[0].project_name.as_deref(),
            Some("A project")
        );
        assert_eq!(
            view.workspace_contexts[0].repository_name.as_deref(),
            Some("A repo")
        );
        assert!(view.live_bindings.is_empty());
        assert!(matches!(view.live_state, LiveState::Unknown));
    }
    #[test]
    fn bindings_require_provider_agent_and_current_incarnation() {
        for (agent, incarnation, expected) in [
            ("codex", "inc", true),
            ("claude", "inc", false),
            ("codex", "stale", false),
        ] {
            let mut snapshot = sample();
            let tab = &mut snapshot.tabs.data.as_mut().unwrap()[0];
            tab.agent_status = Some(json!({"agentType":agent,"providerSession":{"id":"provider"}}));
            tab.incarnation_id = Some(incarnation.into());
            link_sessions(&mut snapshot);
            assert_eq!(
                !snapshot.sessions.data.as_ref().unwrap()[0]
                    .live_bindings
                    .is_empty(),
                expected
            );
        }
    }
    #[test]
    fn remote_credentials_are_never_projected() {
        for input in [
            "https://user:secret@example.test/repo.git?token=secret#secret",
            "ssh://user:secret@example.test/repo",
        ] {
            let mut remote = GitRemote {
                canonical_key: None,
                remote_name: None,
                remote_url: Some(input.into()),
            };
            remote.redact();
            assert!(!remote.remote_url.unwrap().contains("secret"));
        }
    }
}
