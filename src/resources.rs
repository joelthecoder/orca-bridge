//! Point-in-time Orca resource accounting. PTY process trees are not provider conversations.
use crate::{config::Config, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsage {
    /// Native OS CPU percentage; a process tree can exceed 100 percent.
    pub cpu_percent: Option<f64>,
    /// Resident process memory in bytes when processMemoryMetric is rss.
    pub memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppResources {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub main: ResourceUsage,
    pub renderer: ResourceUsage,
    pub other: ResourceUsage,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostResources {
    pub total_memory_bytes: Option<u64>,
    pub free_memory_bytes: Option<u64>,
    pub available_memory_bytes: Option<u64>,
    pub available_memory_source: Option<String>,
    pub used_memory_bytes: Option<u64>,
    pub memory_usage_percent: Option<f64>,
    pub cpu_core_count: Option<u64>,
    pub load_average1m: Option<f64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeResources {
    pub id: String,
    pub display_name: Option<String>,
    pub repo_id: Option<String>,
    pub repo_name: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub session_count: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceBinding {
    Matched,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResources {
    pub terminal: Option<String>,
    pub incarnation_id: Option<String>,
    /// Native diagnostics sessionId is the PTY ID, not a provider conversation ID.
    pub pty_id: String,
    pub worktree_id: String,
    /// Root shell PID; metrics include descendants rather than only this process.
    pub pid: Option<u32>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub binding: ResourceBinding,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub schema_version: u32,
    pub collected_at_ms: u64,
    pub scope: String,
    pub selection: ResourceSelection,
    pub process_memory_metric: String,
    pub runtime_id: Option<String>,
    /// The runtime fence and terminal list were available, not proof every row matched.
    pub bindings_verified: bool,
    pub app: AppResources,
    pub host: HostResources,
    pub worktrees: Vec<WorktreeResources>,
    pub terminals: Vec<TerminalResources>,
    /// Number of rows after any terminal selection and before applying the limit.
    pub terminal_count: usize,
    pub terminals_truncated: bool,
    /// Native app-plus-managed-PTY total, independent of any output filter.
    pub orca_total: ResourceUsage,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSelection {
    pub terminal: Option<String>,
    pub worktree_id: Option<String>,
    pub repo_id: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SortBy {
    Memory,
    Cpu,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

fn usage(value: &Value) -> ResourceUsage {
    ResourceUsage {
        cpu_percent: number(&value["cpu"]),
        memory_bytes: value["memory"].as_u64(),
    }
}

/// Call Orca's supported point-in-time diagnostics endpoint, never a process sweep of our own.
/// Failures in optional identity observations retain the measured rows with unknown bindings.
pub fn capture(config: &Config) -> Result<ResourceSnapshot> {
    let client = RpcClient::new(config);
    let mut errors = Vec::new();
    let before = client.call("status.get", Value::Null).ok();
    if before.is_none() {
        errors.push("Runtime status before resource collection was unavailable".into());
    }
    let native = client
        .call("diagnostics.memory", Value::Null)
        .context("Orca resource diagnostics unavailable")?;
    let terminals = client
        .call(
            "terminal.list",
            json!({"requireFreshPtyLiveness":true,"includeVisualLayouts":false}),
        )
        .ok();
    if terminals.is_none() {
        errors.push("Live terminal metadata was unavailable".into());
    }
    let after = client.call("status.get", Value::Null).ok();
    if after.is_none() {
        errors.push("Runtime status after resource collection was unavailable".into());
    }
    normalize(
        &native,
        terminals.as_ref(),
        before.as_ref().and_then(|v| v["runtimeId"].as_str()),
        after.as_ref().and_then(|v| v["runtimeId"].as_str()),
        errors,
    )
}

/// Select a proven terminal, or rank all measured PTY trees by resident memory.
/// Host, application, worktree and Orca totals retain their global scope.
pub fn select(
    snapshot: ResourceSnapshot,
    terminal: Option<&str>,
    limit: usize,
) -> Result<ResourceSnapshot> {
    select_scoped(snapshot, terminal, None, None, limit, SortBy::Memory)
}

pub fn select_scoped(
    mut snapshot: ResourceSnapshot,
    terminal: Option<&str>,
    worktree_id: Option<&str>,
    repo_id: Option<&str>,
    limit: usize,
    sort: SortBy,
) -> Result<ResourceSnapshot> {
    ensure!(
        (1..=2000).contains(&limit),
        "Resource limit must be between 1 and 2000"
    );
    snapshot.selection = ResourceSelection {
        terminal: terminal.map(str::to_owned),
        worktree_id: worktree_id.map(str::to_owned),
        repo_id: repo_id.map(str::to_owned),
    };
    if worktree_id.is_some() || repo_id.is_some() {
        snapshot.worktrees.retain(|tree| {
            worktree_id.is_none_or(|id| tree.id == id)
                && repo_id.is_none_or(|id| tree.repo_id.as_deref() == Some(id))
        });
        snapshot.terminals.retain(|row| {
            snapshot
                .worktrees
                .iter()
                .any(|tree| tree.id == row.worktree_id)
        });
    }
    if let Some(handle) = terminal {
        snapshot.terminals.retain(|row| {
            row.binding == ResourceBinding::Matched && row.terminal.as_deref() == Some(handle)
        });
        ensure!(
            snapshot.terminals.len() == 1,
            "Expected one verified resource row for this terminal; refresh resource and terminal metadata"
        );
    }
    snapshot.terminals.sort_by(|a, b| {
        (match sort {
            SortBy::Memory => b.memory_bytes.cmp(&a.memory_bytes),
            SortBy::Cpu => b
                .cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
        })
        .then(a.pty_id.cmp(&b.pty_id))
    });
    snapshot.terminal_count = snapshot.terminals.len();
    snapshot.terminals_truncated = snapshot.terminals.len() > limit;
    snapshot.terminals.truncate(limit);
    Ok(snapshot)
}

fn normalize(
    native: &Value,
    terminal_list: Option<&Value>,
    before_runtime: Option<&str>,
    after_runtime: Option<&str>,
    errors: Vec<String>,
) -> Result<ResourceSnapshot> {
    let collected_at_ms = native["collectedAt"]
        .as_u64()
        .context("Unsupported resource snapshot: missing collectedAt timestamp")?;
    ensure!(
        native["app"].is_object() && native["host"].is_object(),
        "Unsupported resource snapshot: expected app and host objects"
    );
    let source_worktrees = native["worktrees"]
        .as_array()
        .context("Unsupported resource snapshot: expected worktrees array")?;
    let listed = terminal_list.and_then(|v| v["terminals"].as_array());
    let same_runtime = before_runtime
        .filter(|id| !id.is_empty())
        .is_some_and(|id| Some(id) == after_runtime);
    let bindings_verified = same_runtime && listed.is_some();
    let mut warnings = vec![
        "PTY process-tree RSS is a point-in-time measurement, not guaranteed reclaimable memory"
            .into(),
    ];
    if !bindings_verified {
        warnings.push("Terminal handles are unavailable because runtime identity could not be verified across collection".into());
    }
    if terminal_list.is_some_and(|v| v["truncated"] == true) {
        warnings.push(
            "The native terminal list was truncated; some resource rows may be unbound".into(),
        );
    }
    let process_memory_metric =
        text(&native["processMemoryMetric"]).unwrap_or_else(|| "unknown".into());
    if process_memory_metric != "rss" {
        warnings
            .push("The native process memory metric is not verified as RSS on this runtime".into());
    }
    let mut worktrees = Vec::new();
    let mut terminals = Vec::new();
    for worktree in source_worktrees {
        let id = text(&worktree["worktreeId"])
            .context("Unsupported resource snapshot: missing worktreeId")?;
        let sessions = worktree["sessions"]
            .as_array()
            .context("Unsupported resource snapshot: expected worktree sessions array")?;
        worktrees.push(WorktreeResources {
            id: id.clone(),
            display_name: text(&worktree["worktreeName"]),
            repo_id: text(&worktree["repoId"]),
            repo_name: text(&worktree["repoName"]),
            cpu_percent: number(&worktree["cpu"]),
            memory_bytes: worktree["memory"].as_u64(),
            session_count: sessions.len(),
        });
        for session in sessions {
            let pty_id = text(&session["sessionId"])
                .context("Unsupported resource snapshot: missing PTY sessionId")?;
            let mut candidates = listed.into_iter().flatten().filter(|row| {
                bindings_verified
                    && row["ptyId"].as_str() == Some(pty_id.as_str())
                    && row["worktreeId"].as_str() == Some(id.as_str())
                    && row["executionHostId"] == "local"
                    && row["connected"] == true
                    && text(&row["handle"]).is_some()
                    && text(&row["incarnationId"]).is_some()
            });
            let first = candidates.next();
            let matched = if candidates.next().is_none() {
                first
            } else {
                None
            };
            terminals.push(TerminalResources {
                terminal: matched.and_then(|v| text(&v["handle"])),
                incarnation_id: matched.and_then(|v| text(&v["incarnationId"])),
                pty_id,
                worktree_id: id.clone(),
                pid: session["pid"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0),
                cpu_percent: number(&session["cpu"]),
                memory_bytes: session["memory"].as_u64(),
                binding: if matched.is_some() {
                    ResourceBinding::Matched
                } else {
                    ResourceBinding::Unavailable
                },
            });
        }
    }
    if bindings_verified
        && terminals
            .iter()
            .any(|row| row.binding == ResourceBinding::Unavailable)
    {
        warnings
            .push("Some measured PTY process trees have no unique current terminal binding".into());
    }
    let app_usage = usage(&native["app"]);
    let host = &native["host"];
    Ok(ResourceSnapshot {
        schema_version: 1,
        collected_at_ms,
        scope: "orca-pty-process-tree".into(),
        selection: ResourceSelection::default(),
        process_memory_metric,
        runtime_id: if same_runtime {
            before_runtime.map(str::to_owned)
        } else {
            None
        },
        bindings_verified,
        app: AppResources {
            cpu_percent: app_usage.cpu_percent,
            memory_bytes: app_usage.memory_bytes,
            main: usage(&native["app"]["main"]),
            renderer: usage(&native["app"]["renderer"]),
            other: usage(&native["app"]["other"]),
        },
        host: HostResources {
            total_memory_bytes: host["totalMemory"].as_u64(),
            free_memory_bytes: host["freeMemory"].as_u64(),
            available_memory_bytes: host["availableMemory"].as_u64(),
            available_memory_source: text(&host["availableMemorySource"]),
            used_memory_bytes: host["usedMemory"].as_u64(),
            memory_usage_percent: number(&host["memoryUsagePercent"]),
            cpu_core_count: host["cpuCoreCount"].as_u64(),
            load_average1m: number(&host["loadAverage1m"]),
        },
        worktrees,
        terminal_count: terminals.len(),
        terminals_truncated: false,
        terminals,
        orca_total: ResourceUsage {
            cpu_percent: number(&native["totalCpu"]),
            memory_bytes: native["totalMemory"].as_u64(),
        },
        warnings,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn capture_uses_native_endpoint_and_fences_terminal_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::write(
            dir.path().join("orca-runtime.json"),
            json!({"runtimeId":"fixture","authToken":"fixture-only","transports":[{"kind":"unix","endpoint":socket_path}]}).to_string(),
        ).unwrap();
        let server = std::thread::spawn(move || {
            for method in [
                "status.get",
                "diagnostics.memory",
                "terminal.list",
                "status.get",
            ] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], method);
                let result = match method {
                    "diagnostics.memory" => {
                        assert!(request["params"].is_null());
                        native()
                    }
                    "terminal.list" => {
                        assert_eq!(request["params"]["requireFreshPtyLiveness"], true);
                        assert_eq!(request["params"]["includeVisualLayouts"], false);
                        terminal_list()
                    }
                    _ => json!({"runtimeId":"fixture"}),
                };
                writeln!(socket, "{}", json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
            }
        });
        let config = Config {
            data_dir: dir.path().into(),
            state_dir: dir.path().join("unused-state"),
            orca_cli: dir.path().join("unused-cli"),
            profile_db_override: None,
        };
        let snapshot = capture(&config).unwrap();
        assert!(snapshot.bindings_verified);
        assert_eq!(
            snapshot.terminals[0].terminal.as_deref(),
            Some("term-fixture")
        );
        assert!(!config.state_dir.exists());
        server.join().unwrap();
    }

    fn native() -> Value {
        json!({
            "collectedAt":1234,"processMemoryMetric":"rss","app":{"cpu":2.5,"memory":4096,"main":{"cpu":1.5,"memory":2048},"history":["private fixture"]},
            "host":{"totalMemory":16384,"availableMemory":8192,"availableMemorySource":"memory-pressure","cpuCoreCount":4},
            "worktrees":[{"worktreeId":"worktree-fixture","worktreeName":"Fixture","repoId":"repo-fixture","repoName":"Repository","cpu":115.5,"memory":8192,"sessions":[
                {"sessionId":"pty-fixture","pid":123,"cpu":115.5,"memory":8192,"paneKey":"private pane fixture","argv":"private argv fixture","transcript":"private transcript fixture"}
            ],"history":["private history fixture"]}],
            "totalCpu":118.0,"totalMemory":12288,"unknown":"private unknown fixture"
        })
    }

    fn terminal_list() -> Value {
        json!({"terminals":[{"ptyId":"pty-fixture","worktreeId":"worktree-fixture","handle":"term-fixture","incarnationId":"incarnation-fixture","executionHostId":"local","connected":true,"title":"private title fixture","preview":"private preview fixture"}],"truncated":false})
    }

    #[test]
    fn normalizes_units_and_binds_only_native_pty_identity_without_private_fields() {
        let snapshot = normalize(
            &native(),
            Some(&terminal_list()),
            Some("runtime-fixture"),
            Some("runtime-fixture"),
            vec![],
        )
        .unwrap();
        assert!(snapshot.bindings_verified);
        assert_eq!(
            snapshot.terminals[0].terminal.as_deref(),
            Some("term-fixture")
        );
        assert_eq!(snapshot.terminals[0].pid, Some(123));
        assert_eq!(snapshot.terminals[0].cpu_percent, Some(115.5));
        assert_eq!(snapshot.terminals[0].memory_bytes, Some(8192));
        assert_eq!(snapshot.host.free_memory_bytes, None);
        assert_eq!(snapshot.worktrees[0].session_count, 1);
        assert_eq!(snapshot.orca_total.memory_bytes, Some(12288));
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("private"));
        assert!(encoded.contains("memoryBytes"));
    }

    #[test]
    fn retains_measurements_and_unknowns_when_runtime_or_list_is_unavailable() {
        for (list, before, after) in [
            (Some(terminal_list()), Some("before"), Some("after")),
            (Some(terminal_list()), None, Some("runtime")),
            (None, Some("runtime"), Some("runtime")),
            (
                Some(json!({"terminals":null})),
                Some("runtime"),
                Some("runtime"),
            ),
        ] {
            let snapshot = normalize(
                &native(),
                list.as_ref(),
                before,
                after,
                vec!["fixture observation unavailable".into()],
            )
            .unwrap();
            assert!(!snapshot.bindings_verified);
            assert_eq!(snapshot.terminals[0].terminal, None);
            assert_eq!(snapshot.terminals[0].incarnation_id, None);
            assert_eq!(snapshot.terminals[0].binding, ResourceBinding::Unavailable);
            assert_eq!(snapshot.terminals[0].memory_bytes, Some(8192));
            assert_eq!(snapshot.errors.len(), 1);
        }
    }

    #[test]
    fn rejects_path_title_provider_id_and_ambiguous_or_stale_bindings() {
        for change in [
            json!({"ptyId":"different"}),
            json!({"worktreeId":"different"}),
            json!({"incarnationId":""}),
            json!({"executionHostId":"remote"}),
            json!({"connected":false}),
            json!({"connected":null}),
        ] {
            let mut list = terminal_list();
            for (key, value) in change.as_object().unwrap() {
                list["terminals"][0][key] = value.clone();
            }
            let snapshot = normalize(
                &native(),
                Some(&list),
                Some("runtime"),
                Some("runtime"),
                vec![],
            )
            .unwrap();
            assert_eq!(snapshot.terminals[0].binding, ResourceBinding::Unavailable);
        }
        let mut list = terminal_list();
        list["terminals"][0]
            .as_object_mut()
            .unwrap()
            .remove("connected");
        let snapshot = normalize(
            &native(),
            Some(&list),
            Some("runtime"),
            Some("runtime"),
            vec![],
        )
        .unwrap();
        assert_eq!(snapshot.terminals[0].binding, ResourceBinding::Unavailable);
        let mut list = terminal_list();
        let duplicate = list["terminals"][0].clone();
        list["terminals"].as_array_mut().unwrap().push(duplicate);
        let snapshot = normalize(
            &native(),
            Some(&list),
            Some("runtime"),
            Some("runtime"),
            vec![],
        )
        .unwrap();
        assert_eq!(snapshot.terminals[0].terminal, None);
    }

    #[test]
    fn selection_ranks_known_memory_and_preserves_global_totals_and_counts() {
        let mut snapshot = normalize(
            &native(),
            Some(&terminal_list()),
            Some("runtime"),
            Some("runtime"),
            vec![],
        )
        .unwrap();
        let mut unknown = snapshot.terminals[0].clone();
        unknown.pty_id = "unknown".into();
        unknown.memory_bytes = None;
        let mut large = snapshot.terminals[0].clone();
        large.pty_id = "large".into();
        large.memory_bytes = Some(16384);
        snapshot.terminals.extend([unknown, large]);
        let selected = select(snapshot, None, 2).unwrap();
        assert_eq!(selected.terminal_count, 3);
        assert!(selected.terminals_truncated);
        assert_eq!(selected.terminals.len(), 2);
        assert_eq!(selected.terminals[0].pty_id, "large");
        assert_eq!(selected.terminals[1].memory_bytes, Some(8192));
        assert_eq!(selected.orca_total.memory_bytes, Some(12288));
        assert_eq!(selected.worktrees.len(), 1);
    }

    #[test]
    fn selection_requires_one_verified_handle_and_a_bounded_positive_limit() {
        let snapshot = normalize(
            &native(),
            Some(&terminal_list()),
            Some("runtime"),
            Some("runtime"),
            vec![],
        )
        .unwrap();
        assert!(select(snapshot.clone(), None, 0).is_err());
        assert!(select(snapshot.clone(), None, 2001).is_err());
        assert!(select(snapshot.clone(), Some("unknown"), 10).is_err());
        let selected = select(snapshot.clone(), Some("term-fixture"), 10).unwrap();
        assert_eq!(selected.terminal_count, 1);
        assert!(!selected.terminals_truncated);
        let mut ambiguous = snapshot.clone();
        ambiguous.terminals.push(snapshot.terminals[0].clone());
        assert!(select(ambiguous, Some("term-fixture"), 10).is_err());
        let mut unbound = snapshot;
        unbound.terminals[0].binding = ResourceBinding::Unavailable;
        assert!(select(unbound, Some("term-fixture"), 10).is_err());
    }

    #[test]
    fn rejects_unsupported_native_shapes_and_preserves_missing_counters() {
        for source in [
            json!({}),
            json!({"collectedAt":1234,"app":{},"host":{},"worktrees":null}),
        ] {
            assert!(normalize(&source, None, None, None, vec![]).is_err());
        }
        let mut source = native();
        source["worktrees"][0]["sessions"][0]["cpu"] = json!(-1);
        source["worktrees"][0]["sessions"][0]["memory"] = Value::Null;
        let snapshot = normalize(&source, None, None, None, vec![]).unwrap();
        assert_eq!(snapshot.terminals[0].cpu_percent, None);
        assert_eq!(snapshot.terminals[0].memory_bytes, None);
    }
}
