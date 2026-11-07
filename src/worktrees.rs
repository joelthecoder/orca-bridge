//! Manual worktree inspection and guarded removal through Orca's native contract.
//! Disk sizes are allocated-space estimates, not promises of reclaimable space.
use crate::{
    actions,
    config::Config,
    errors::{self, BridgeError, ErrorCode, Outcome},
    rpc::RpcClient,
};
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt,
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_CAP: usize = 1024 * 1024;
const REAP_RESERVE: Duration = Duration::from_millis(30);

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeIdentity {
    pub id: Option<String>,
    pub instance_id: Option<String>,
    pub host_id: Option<String>,
    pub path: Option<String>,
    pub name: Option<String>,
    pub repo_id: Option<String>,
    pub is_main_worktree: Option<bool>,
    pub branch: Option<String>,
    /// Native opaque instance-bound selector; null means no proven local selector.
    pub worktree_selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiskMeasurement {
    pub available: bool,
    /// Allocated-block estimate, not guaranteed reclaimable bytes.
    pub allocated_bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitMeasurement {
    pub available: bool,
    pub dirty: Option<bool>,
    pub tracked_changes: Option<usize>,
    /// Porcelain entries; an untracked directory can represent multiple files.
    pub untracked_entries: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeUsageRow {
    #[serde(flatten)]
    pub identity: WorktreeIdentity,
    pub disk: DiskMeasurement,
    pub git: GitMeasurement,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostScope {
    pub host_ids: Option<Vec<String>>,
    pub omitted_host_ids: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestedScope {
    pub worktree: Option<String>,
    pub repo: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub worktrees: Vec<WorktreeUsageRow>,
    pub inventory_truncated: bool,
    pub scan_limited: bool,
    /// Count of successful disk measurements, independent of Git availability.
    pub measured_count: usize,
    pub inventory_total_count: Option<usize>,
    pub host_scope: Option<HostScope>,
    pub requested_scope: RequestedScope,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryRow {
    #[serde(flatten)]
    pub identity: WorktreeIdentity,
    pub source: String,
    /// Whether this row appeared in this native inventory page, not registration proof.
    pub listed_by_orca: bool,
    /// Native metadata only; Git state never implies archive state.
    pub is_archived: Option<bool>,
    pub detached_head: Option<bool>,
    pub locked: Option<bool>,
    pub prunable: Option<bool>,
    pub path_exists: Option<bool>,
    pub git_status_available: bool,
    pub repo_external_visibility: Option<String>,
    pub native_resolution: Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInventory {
    pub schema_version: u32,
    pub captured_at: String,
    pub worktrees: Vec<InventoryRow>,
    pub include_external: bool,
    pub requested_repo: Option<String>,
    pub inventory_truncated: bool,
    pub scan_limited: bool,
    pub native_total_count: Option<usize>,
    pub host_scope: Option<HostScope>,
    pub discovered_count: usize,
    pub git_discovered_count: usize,
    pub excluded_external_count: usize,
    pub repository_count: usize,
    pub inspected_repo_count: usize,
    pub unavailable_repo_count: usize,
    pub unavailable_repos: Vec<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalBlocker {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspection_error: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalEffects {
    pub native_branch_deletion_possible: bool,
    pub branch: Option<String>,
    pub run_archive_hooks: bool,
    pub allow_failed_archive_hook: bool,
    pub force_permits_dirty_worktree: bool,
    pub stops_terminals: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalChecks {
    pub git: GitMeasurement,
    pub git_root_verified: bool,
    /// Version-dependent native liveness projection; never includes previews/prompts.
    pub terminals: Value,
    pub runtime_consistent: Option<bool>,
    pub identity_refreshed: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalPlan {
    pub schema_version: u32,
    pub action: String,
    pub dry_run: bool,
    pub checked_at: String,
    pub requested_selector: String,
    pub allowed: bool,
    pub target: Option<WorktreeIdentity>,
    pub blockers: Vec<RemovalBlocker>,
    pub checks: RemovalChecks,
    pub effects: RemovalEffects,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RemovalOutcome {
    Verified,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalVerification {
    pub path_absent: Option<bool>,
    pub instance_absent: Option<bool>,
    pub runtime_consistent: bool,
    pub errors: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemovalReceipt {
    pub schema_version: u32,
    pub action: String,
    pub operation_id: String,
    pub completed_at: String,
    pub target: WorktreeIdentity,
    pub acknowledged: bool,
    pub verified: bool,
    pub outcome: RemovalOutcome,
    pub effects: RemovalEffects,
    pub native_result: Value,
    pub verification: RemovalVerification,
}

#[derive(Debug)]
struct InspectionLimit(&'static str);
impl fmt::Display for InspectionLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for InspectionLimit {}

fn remaining(deadline: Instant) -> Result<Duration> {
    let duration = deadline.saturating_duration_since(Instant::now());
    ensure!(!duration.is_zero(), InspectionLimit("Inspection timed out"));
    Ok(duration)
}

fn rpc(config: &Config, deadline: Instant, method: &str, params: Value) -> Result<Value> {
    RpcClient::new(config)
        .with_timeout(remaining(deadline)?.min(COMMAND_TIMEOUT))
        .call(method, params)
}

fn stop_group(child: &mut Child, deadline: Instant) {
    // Every inspection owns its process group. Kill descendants as well: a helper
    // must not keep stdout open after its parent exits or a timeout expires.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// Bounded capture without reader threads or blocking waits on inherited pipes.
fn command_output(mut command: Command, deadline: Instant, cap: usize) -> Result<Vec<u8>> {
    let now = Instant::now();
    let cutoff = deadline.min(now + COMMAND_TIMEOUT);
    ensure!(
        cutoff.saturating_duration_since(now) > REAP_RESERVE,
        InspectionLimit("Inspection timed out")
    );
    let work_cutoff = cutoff - REAP_RESERVE;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Cannot start inspection command")?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .context("Inspection stdout unavailable")?;
        let fd = stdout.as_raw_fd();
        // This descriptor is owned by stdout for the duration of both calls.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(flags >= 0, "Cannot inspect subprocess output flags");
        ensure!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Cannot make subprocess output nonblocking"
        );
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut eof = false;
        let mut exited_once = false;
        loop {
            ensure!(
                Instant::now() < work_cutoff,
                InspectionLimit("Inspection timed out")
            );
            loop {
                ensure!(
                    Instant::now() < work_cutoff,
                    InspectionLimit("Inspection timed out")
                );
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(count) => {
                        ensure!(
                            output.len().saturating_add(count) <= cap,
                            InspectionLimit("Inspection output limit exceeded")
                        );
                        output.extend_from_slice(&buffer[..count]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error).context("Cannot read inspection output"),
                }
            }
            if let Some(status) = child.try_wait().context("Cannot inspect command status")? {
                ensure!(status.success(), "Inspection command failed");
                if eof {
                    return Ok(output);
                }
                // The child can exit between the nonblocking read and try_wait.
                // Give the pipe one more drain before diagnosing an inheriting helper.
                ensure!(!exited_once, "Inspection command left output open");
                exited_once = true;
            }
            std::thread::sleep(
                Duration::from_millis(5).min(work_cutoff.saturating_duration_since(Instant::now())),
            );
        }
    })();
    if result.is_err() {
        stop_group(&mut child, cutoff);
    }
    result
}

fn local(row: &Value) -> bool {
    row["hostId"].as_str() == Some("local")
        && row
            .pointer("/identity/executionHostId")
            .and_then(Value::as_str)
            .is_none_or(|host| host == "local")
}

fn local_path(row: &Value) -> Result<PathBuf> {
    ensure!(local(row), "Only verified local worktrees can be inspected");
    let path = Path::new(row["path"].as_str().context("Worktree path is unknown")?);
    ensure!(path.is_absolute(), "Worktree path must be absolute");
    let canonical = path
        .canonicalize()
        .context("Worktree directory is unavailable")?;
    ensure!(canonical.is_dir(), "Worktree path is not a directory");
    Ok(canonical)
}

fn du_bytes(output: &[u8]) -> Result<u64> {
    let digits = output
        .split(|byte| byte.is_ascii_whitespace())
        .find(|part| !part.is_empty())
        .context("Disk inspection returned no size")?;
    ensure!(
        digits.iter().all(u8::is_ascii_digit),
        "Invalid disk inspection size"
    );
    std::str::from_utf8(digits)?
        .parse::<u64>()?
        .checked_mul(1024)
        .context("Disk inspection size overflow")
}

fn disk(path: &Path, deadline: Instant) -> Result<u64> {
    let mut command = Command::new("/usr/bin/du");
    command.args([OsStr::new("-skx"), path.as_os_str()]);
    du_bytes(&command_output(command, deadline, 16 * 1024)?)
}

fn git_command(path: &Path) -> Command {
    let mut command = Command::new("git");
    command.env("GIT_OPTIONAL_LOCKS", "0").args([
        OsStr::new("-c"),
        OsStr::new("core.fsmonitor=false"),
        OsStr::new("-C"),
        path.as_os_str(),
    ]);
    command
}

fn status_counts(output: &[u8]) -> Result<(usize, usize)> {
    ensure!(
        output.is_empty() || output.last() == Some(&0),
        "Incomplete Git status output"
    );
    let mut tracked = 0;
    let mut untracked = 0;
    let mut entries = output.split(|byte| *byte == 0);
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            ensure!(
                entries.all(|rest| rest.is_empty()),
                "Invalid Git status output"
            );
            break;
        }
        ensure!(
            entry.len() >= 4 && entry[2] == b' ',
            "Invalid Git status output"
        );
        if &entry[..2] == b"??" {
            untracked += 1;
        } else if &entry[..2] != b"!!" {
            tracked += 1;
            if entry[..2].iter().any(|byte| matches!(byte, b'R' | b'C')) {
                ensure!(
                    entries.next().is_some_and(|path| !path.is_empty()),
                    "Incomplete Git rename status"
                );
            }
        }
    }
    Ok((tracked, untracked))
}

fn git_status(path: &Path, deadline: Instant) -> Result<(usize, usize)> {
    let mut command = git_command(path);
    command.args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"]);
    status_counts(&command_output(command, deadline, OUTPUT_CAP)?)
}

fn limited(error: &anyhow::Error) -> bool {
    error.downcast_ref::<InspectionLimit>().is_some()
}

fn projection(row: &Value) -> Value {
    json!({
        "id":row.get("id"), "instanceId":row.get("instanceId"),
        "hostId":row.get("hostId"), "path":row.get("path"),
        "name":row.get("displayName"), "repoId":row.get("repoId"),
        "isMainWorktree":row.get("isMainWorktree"), "branch":row.get("branch"),
        "worktreeSelector":exact_selector(row).ok(),
    })
}

fn unavailable_disk(error: &str) -> Value {
    json!({"available":false,"allocatedBytes":null,"error":error})
}

fn unavailable_git(error: &str) -> Value {
    json!({"available":false,"dirty":null,"trackedChanges":null,"untrackedEntries":null,"error":error})
}

/// Inspect selected local directories; never selects or deletes cleanup candidates.
pub fn usage(
    config: &Config,
    selector: Option<&str>,
    limit: usize,
    timeout: Duration,
) -> Result<Value> {
    usage_scoped(config, selector, None, limit, timeout)
}

/// Inspect a selected tree or one repository's bounded inventory.
pub fn usage_scoped(
    config: &Config,
    selector: Option<&str>,
    repo: Option<&str>,
    limit: usize,
    timeout: Duration,
) -> Result<Value> {
    ensure!(
        selector.is_none() || repo.is_none(),
        "--worktree and --repo cannot be combined"
    );
    ensure!(
        repo.is_none_or(|value| !value.trim().is_empty()),
        "Supply a repository selector"
    );
    ensure!(
        (1..=200).contains(&limit),
        "Worktree limit must be between 1 and 200"
    );
    ensure!(!timeout.is_zero(), "Inspection timeout must be positive");
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("Inspection timeout is too large")?;
    let native = if let Some(selector) = selector {
        ensure!(!selector.trim().is_empty(), "Supply a worktree selector");
        rpc(
            config,
            deadline,
            "worktree.show",
            json!({"worktree":selector}),
        )?
    } else {
        let mut params = json!({"limit":limit});
        if let Some(repo) = repo {
            params["repo"] = repo.into();
        }
        rpc(config, deadline, "worktree.list", params)?
    };
    let native_rows = if selector.is_some() {
        vec![native.get("worktree").context("Missing worktree")?.clone()]
    } else {
        native["worktrees"]
            .as_array()
            .context("Missing worktrees")?
            .clone()
    };
    let inventory_truncated = selector.is_none()
        && (native["truncated"].as_bool() != Some(false) || native_rows.len() > limit);
    let host_scope = native.get("hostScope").map(|scope| {
        json!({"hostIds":scope.get("hostIds"),"omittedHostIds":scope.get("omittedHostIds")})
    });
    let mut rows = Vec::new();
    let mut scan_limited = false;
    let mut measured_count = 0;
    for native_row in native_rows.iter().take(limit) {
        let mut row = projection(native_row);
        match local_path(native_row) {
            Err(error) => {
                row["disk"] = unavailable_disk(&error.to_string());
                row["git"] = unavailable_git(&error.to_string());
            }
            Ok(path) => {
                match disk(&path, deadline) {
                    Ok(bytes) => {
                        row["disk"] = json!({"available":true,"allocatedBytes":bytes,"error":null});
                        measured_count += 1;
                    }
                    Err(error) => {
                        scan_limited |= limited(&error);
                        row["disk"] = unavailable_disk(&error.to_string());
                    }
                }
                match git_status(&path, deadline) {
                    Ok((tracked, untracked)) => {
                        row["git"] = json!({"available":true,"dirty":tracked>0 || untracked>0,
                            "trackedChanges":tracked,"untrackedEntries":untracked,"error":null});
                    }
                    Err(error) => {
                        scan_limited |= limited(&error);
                        row["git"] = unavailable_git(&error.to_string());
                    }
                }
            }
        }
        rows.push(row);
    }
    Ok(
        json!({"schemaVersion":1,"capturedAt":chrono::Utc::now().to_rfc3339(),
        "worktrees":rows,"inventoryTruncated":inventory_truncated,"scanLimited":scan_limited,
        "measuredCount":measured_count,"inventoryTotalCount":native.get("totalCount"),
        "hostScope":host_scope,"requestedScope":{"worktree":selector,"repo":repo}}),
    )
}

#[derive(Debug)]
struct GitWorktree {
    path: PathBuf,
    branch: Option<String>,
    detached: Option<bool>,
    locked: bool,
    prunable: bool,
    main: bool,
}

/// Git -z mode leaves paths/reasons unquoted and separates fields with NULs.
/// In particular, a newline inside a path is not a record separator.
fn git_worktrees(output: &[u8]) -> Result<Vec<GitWorktree>> {
    use std::os::unix::ffi::OsStrExt;
    ensure!(
        output.is_empty() || output.last() == Some(&0),
        "Incomplete Git worktree inventory"
    );
    let mut records = Vec::new();
    let mut fields: Vec<&[u8]> = Vec::new();
    for field in output
        .split(|byte| *byte == 0)
        .chain(std::iter::once(&[][..]))
    {
        if !field.is_empty() {
            fields.push(field);
            continue;
        }
        if fields.is_empty() {
            continue;
        }
        let paths = fields
            .iter()
            .filter_map(|field| field.strip_prefix(b"worktree "))
            .collect::<Vec<_>>();
        ensure!(
            paths.len() == 1 && !paths[0].is_empty(),
            "Git inventory record has no unique path"
        );
        let path = PathBuf::from(OsStr::from_bytes(paths[0]));
        ensure!(path.is_absolute(), "Git inventory path is not absolute");
        let branch = fields
            .iter()
            .find_map(|field| field.strip_prefix(b"branch "))
            .map(std::str::from_utf8)
            .transpose()?
            .map(str::to_owned);
        let detached = fields.contains(&b"detached".as_slice());
        records.push(GitWorktree {
            path,
            detached: if detached {
                Some(true)
            } else {
                branch.as_ref().map(|_| false)
            },
            branch,
            locked: fields
                .iter()
                .any(|field| *field == b"locked" || field.starts_with(b"locked ")),
            prunable: fields
                .iter()
                .any(|field| *field == b"prunable" || field.starts_with(b"prunable ")),
            main: records.is_empty(),
        });
        fields.clear();
    }
    Ok(records)
}

fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn visibility(repo: &Value) -> Option<&str> {
    repo["externalWorktreeVisibility"]
        .as_str()
        .filter(|value| matches!(*value, "show" | "hide" | "inherit"))
}

fn native_inventory_row(row: &Value) -> Value {
    let mut result = projection(row);
    result["source"] = "orca-native".into();
    result["listedByOrca"] = true.into();
    result["isArchived"] = row["isArchived"].as_bool().map_or(Value::Null, Value::from);
    for field in [
        "detachedHead",
        "locked",
        "prunable",
        "pathExists",
        "repoExternalVisibility",
    ] {
        result[field] = Value::Null;
    }
    if local(row) {
        result["pathExists"] = row["path"]
            .as_str()
            .filter(|path| Path::new(path).is_absolute())
            .and_then(|path| Path::new(path).try_exists().ok())
            .map_or(Value::Null, Value::from);
    }
    result["gitStatusAvailable"] = false.into();
    result["nativeResolution"] = json!({"available":true,"identityVerified":result["worktreeSelector"].is_string(),"error":null});
    result
}

fn annotate_git(row: &mut Value, entry: &GitWorktree, repo: &Value) {
    row["detachedHead"] = entry.detached.map_or(Value::Null, Value::from);
    row["locked"] = entry.locked.into();
    row["prunable"] = entry.prunable.into();
    row["pathExists"] = entry
        .path
        .try_exists()
        .ok()
        .map_or(Value::Null, Value::from);
    row["gitStatusAvailable"] = true.into();
    row["isMainWorktree"] = entry.main.into();
    row["repoExternalVisibility"] = visibility(repo).map_or(Value::Null, Value::from);
}

fn git_discovered_row(entry: &GitWorktree, repo: &Value) -> Value {
    let mut row = projection(
        &json!({"hostId":"local","repoId":repo["id"],"path":entry.path.to_str(),
        "branch":entry.branch.as_deref().map(|branch|branch.strip_prefix("refs/heads/").unwrap_or(branch)),"isMainWorktree":entry.main}),
    );
    row["source"] = "git-discovered".into();
    row["listedByOrca"] = false.into();
    row["isArchived"] = Value::Null;
    row["nativeResolution"] = json!({"available":false,"identityVerified":false,"error":null});
    annotate_git(&mut row, entry, repo);
    row
}

fn hydrate_git_identity(
    config: &Config,
    deadline: Instant,
    row: &mut Value,
    repo: &Value,
    entry: &GitWorktree,
) -> bool {
    let Some(path) = entry.path.to_str() else {
        row["nativeResolution"]["error"] = json!({"code":"non_utf8_path","message":"Native identity resolution requires a UTF-8 path"});
        return false;
    };
    match rpc(
        config,
        deadline,
        "worktree.show",
        json!({"worktree":format!("path:{path}")}),
    ) {
        Ok(shown) => {
            let found = shown.get("worktree").filter(|found| {
                local(found)
                    && found["repoId"] == repo["id"]
                    && found["path"]
                        .as_str()
                        .is_some_and(|path| path_key(Path::new(path)) == path_key(&entry.path))
            });
            if let Some(found) = found {
                let identity = projection(found);
                for key in [
                    "id",
                    "instanceId",
                    "hostId",
                    "name",
                    "repoId",
                    "worktreeSelector",
                ] {
                    row[key] = identity[key].clone();
                }
                row["isArchived"] = found["isArchived"]
                    .as_bool()
                    .map_or(Value::Null, Value::from);
                row["nativeResolution"] = json!({"available":true,"identityVerified":row["worktreeSelector"].is_string(),"error":null});
            } else {
                row["nativeResolution"]["error"] = json!({"code":"identity_mismatch","message":"Native resolution did not match the Git repository and path"});
            }
            false
        }
        Err(error) => {
            row["nativeResolution"]["error"] = errors::envelope(&error)["error"].clone();
            limited(&error)
                || error.downcast_ref::<BridgeError>().is_some_and(|error| {
                    matches!(
                        error.code,
                        ErrorCode::RpcTimeout | ErrorCode::CommandTimeout
                    )
                })
        }
    }
}

fn inventory_repo_path(repo: &Value, local_repos: &HashSet<String>) -> Result<PathBuf> {
    let id = repo["id"]
        .as_str()
        .context("Repository ID is unavailable")?;
    ensure!(repo["kind"] == "git", "Repository is not verified Git");
    ensure!(
        match repo["hostId"].as_str() {
            Some("local") => true,
            Some(_) => false,
            None => local_repos.contains(id),
        },
        "Repository execution host is not verified local"
    );
    let path = Path::new(
        repo["path"]
            .as_str()
            .context("Repository path is unavailable")?,
    );
    ensure!(path.is_absolute(), "Repository path must be absolute");
    let path = path
        .canonicalize()
        .context("Repository directory is unavailable")?;
    ensure!(path.is_dir(), "Repository path is not a directory");
    Ok(path)
}

/// Read-only inventory. Hidden/native-list-omitted Git rows are opt-in, and Git
/// lock/detach/prunable state never constitutes permission or a safe-prune verdict.
pub fn inventory(
    config: &Config,
    repo: Option<&str>,
    limit: usize,
    timeout: Duration,
    include_external: bool,
) -> Result<Value> {
    ensure!(
        (1..=200).contains(&limit),
        "Worktree inventory limit must be between 1 and 200"
    );
    ensure!(!timeout.is_zero(), "Inventory timeout must be positive");
    ensure!(
        repo.is_none_or(|value| !value.trim().is_empty()),
        "Supply a repository selector"
    );
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("Inventory timeout is too large")?;
    let mut params = json!({"limit":limit});
    if let Some(repo) = repo {
        params["repo"] = repo.into();
    }
    let native = rpc(config, deadline, "worktree.list", params)?;
    let native_rows = native["worktrees"]
        .as_array()
        .context("Missing native worktree inventory")?;
    let mut rows = native_rows
        .iter()
        .take(limit)
        .map(native_inventory_row)
        .collect::<Vec<_>>();
    let mut native_indexes = HashMap::new();
    let mut local_repos = HashSet::new();
    for (index, row) in native_rows.iter().take(limit).enumerate() {
        if !local(row) {
            continue;
        }
        if let Some(repo) = row["repoId"].as_str() {
            local_repos.insert(repo.to_owned());
        }
        if let (Some(repo), Some(path)) = (row["repoId"].as_str(), row["path"].as_str())
            && Path::new(path).is_absolute()
        {
            native_indexes.insert((repo.to_owned(), path_key(Path::new(path))), index);
        }
    }
    let repo_result = if let Some(repo) = repo {
        rpc(config, deadline, "repo.show", json!({"repo":repo})).and_then(|result| {
            result
                .get("repo")
                .cloned()
                .map(|row| vec![row])
                .context("Missing repository")
        })
    } else {
        rpc(config, deadline, "repo.list", Value::Null).and_then(|result| {
            result["repos"]
                .as_array()
                .cloned()
                .context("Missing repositories")
        })
    };
    let mut unavailable = Vec::new();
    let mut scan_limited = false;
    let mut inspected = 0;
    let mut repository_count = 0;
    let mut git_discovered_count = 0;
    let mut unique_repos = HashSet::new();
    let mut git_seen = HashSet::new();
    match repo_result {
        Err(error) => {
            scan_limited = true;
            unavailable.push(json!({"repoId":null,"error":errors::envelope(&error)["error"]}));
        }
        Ok(repositories) => {
            for repository in repositories {
                let id = repository["id"].as_str().map(str::to_owned);
                if id
                    .as_ref()
                    .is_some_and(|id| !unique_repos.insert(id.clone()))
                {
                    continue;
                }
                repository_count += 1;
                let inspected_repo = (|| -> Result<()> {
                    ensure!(
                        repository_count <= 200,
                        InspectionLimit("Repository inspection limit exceeded")
                    );
                    remaining(deadline)?;
                    let path = inventory_repo_path(&repository, &local_repos)?;
                    let mut command = git_command(&path);
                    command.args(["worktree", "list", "--porcelain", "-z"]);
                    let entries = git_worktrees(&command_output(command, deadline, OUTPUT_CAP)?)?;
                    inspected += 1;
                    for entry in entries {
                        let Some(id) = id.as_deref() else {
                            continue;
                        };
                        let key = (id.to_owned(), path_key(&entry.path));
                        if !git_seen.insert(key.clone()) {
                            continue;
                        }
                        if let Some(index) = native_indexes.get(&key) {
                            annotate_git(&mut rows[*index], &entry, &repository);
                        } else {
                            git_discovered_count += 1;
                            if include_external {
                                let mut row = git_discovered_row(&entry, &repository);
                                if rows.len() < limit {
                                    scan_limited |= hydrate_git_identity(
                                        config,
                                        deadline,
                                        &mut row,
                                        &repository,
                                        &entry,
                                    );
                                }
                                rows.push(row);
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = inspected_repo {
                    scan_limited |= limited(&error);
                    unavailable
                        .push(json!({"repoId":id,"error":errors::envelope(&error)["error"]}));
                }
                if let Some(id) = id.as_deref() {
                    for row in rows.iter_mut().filter(|row| row["repoId"] == id) {
                        row["repoExternalVisibility"] =
                            visibility(&repository).map_or(Value::Null, Value::from);
                    }
                }
            }
        }
    }
    let discovered_count = rows.len();
    let inventory_truncated = native["truncated"].as_bool() != Some(false)
        || native_rows.len() > limit
        || rows.len() > limit;
    rows.truncate(limit);
    let host_scope=native.get("hostScope").map(|scope|json!({"hostIds":scope.get("hostIds"),"omittedHostIds":scope.get("omittedHostIds")}));
    Ok(
        json!({"schemaVersion":1,"capturedAt":chrono::Utc::now().to_rfc3339(),"worktrees":rows,
        "includeExternal":include_external,"requestedRepo":repo,"inventoryTruncated":inventory_truncated,"scanLimited":scan_limited,
        "nativeTotalCount":native.get("totalCount"),"hostScope":host_scope,"discoveredCount":discovered_count,
        "gitDiscoveredCount":git_discovered_count,"excludedExternalCount":if include_external {0}else{git_discovered_count},
        "repositoryCount":repository_count,"inspectedRepoCount":inspected,"unavailableRepoCount":unavailable.len(),"unavailableRepos":unavailable,
        "warnings":["Git-discovered means absent from this native inventory page, not proof of being unregistered.",
            "Archived metadata is preserved only when returned or resolved by Orca; its CLI advertises no include-archived inventory flag.",
            "Detached, locked, prunable or missing paths are inspection facts, never a safe-to-delete verdict.",
            "Missing worktree paths can leave Git administrative metadata; inventory does not promise reclaimable disk space."]}),
    )
}

fn remove_selector(selector: &str) -> Result<()> {
    ensure!(
        !selector.trim().is_empty()
            && !matches!(selector.trim(), "active" | "current" | "all" | "*"),
        "Removal requires an explicit worktree selector, not active/current/all"
    );
    Ok(())
}

fn protected(row: &Value) -> Result<()> {
    ensure!(local(row), "Removal supports only verified local worktrees");
    ensure!(
        row["isMainWorktree"] == false,
        "Main or unknown worktrees cannot be removed"
    );
    ensure!(
        row["isBare"] == false,
        "Bare or unknown repositories cannot be removed"
    );
    Ok(())
}

fn exact_selector(row: &Value) -> Result<String> {
    ensure!(local(row), "Worktree identity is not verified local");
    let instance = row["instanceId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Worktree instance is unknown")?;
    ensure!(
        row.pointer("/identity/instanceId").and_then(Value::as_str) == Some(instance),
        "Worktree identity is not instance-bound"
    );
    ensure!(
        row.pointer("/identity/executionHostId")
            .and_then(Value::as_str)
            == Some("local"),
        "Worktree identity host is unknown"
    );
    let key = row
        .pointer("/identity/key")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("Exact worktree identity is unavailable")?;
    Ok(format!("identity:{key}"))
}

fn safe_git_root(path: &Path, deadline: Instant) -> Result<()> {
    let mut command = git_command(path);
    command.args(["rev-parse", "--show-toplevel"]);
    let root = command_output(command, deadline, OUTPUT_CAP)?;
    let root = std::str::from_utf8(&root)?.trim_end_matches(['\n', '\r']);
    ensure!(
        Path::new(root).canonicalize()? == path,
        "Removal target is not the Git worktree root"
    );
    let mut command = git_command(path);
    command.args(["worktree", "list", "--porcelain", "-z"]);
    let output = command_output(command, deadline, OUTPUT_CAP)?;
    let main = output
        .split(|byte| *byte == 0)
        .next()
        .and_then(|field| field.strip_prefix(b"worktree "))
        .context("Git main worktree is unknown")?;
    use std::os::unix::ffi::OsStrExt;
    ensure!(
        Path::new(OsStr::from_bytes(main)).canonicalize()? != path,
        "The base Git repository cannot be removed"
    );
    Ok(())
}

fn idle(ps: &Value, worktree: &Value) -> Result<()> {
    ensure!(
        ps["truncated"] == false,
        "Terminal inventory is incomplete; removal refused"
    );
    let scope = ps
        .get("hostScope")
        .context("Terminal inventory host scope is unknown")?;
    ensure!(
        scope["hostIds"]
            .as_array()
            .is_some_and(|hosts| hosts.iter().any(|host| host == "local")),
        "Terminal inventory does not cover the local host"
    );
    ensure!(
        scope["omittedHostIds"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "Terminal inventory omits hosts; removal refused"
    );
    let id = worktree["id"].as_str().context("Worktree ID is unknown")?;
    let matches = ps["worktrees"]
        .as_array()
        .context("Missing terminal inventory")?
        .iter()
        .filter(|row| row["worktreeId"] == id)
        .collect::<Vec<_>>();
    ensure!(
        matches.len() == 1,
        "Worktree terminal inventory is missing or ambiguous"
    );
    let row = matches[0];
    ensure!(
        row["hostId"] == "local" && row["worktreeInstanceId"] == worktree["instanceId"],
        "Worktree terminal inventory identity changed"
    );
    ensure!(
        row["hasAttachedPty"] == false
            && row["liveTerminalCount"] == 0
            && row["unverifiableTerminalCount"] == 0
            && matches!(row["terminalVerdict"].as_str(), Some("none" | "exited")),
        "Worktree has live or unverifiable terminals; stop them explicitly before removal"
    );
    Ok(())
}

struct CheckedRemoval {
    plan: Value,
    exact: Option<String>,
    path: Option<PathBuf>,
    runtime: Option<String>,
}

fn blocker(plan: &mut Value, code: &str, message: &str) {
    let blockers = plan["blockers"].as_array_mut().expect("owned blocker list");
    if !blockers.iter().any(|entry| entry["code"] == code) {
        blockers.push(json!({"code":code,"message":message}));
    }
}

fn inspection_blocker(plan: &mut Value, code: &str, message: &str, error: &anyhow::Error) {
    blocker(plan, code, message);
    if let Some(entry) = plan["blockers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["code"] == code)
    {
        entry["inspectionError"] = errors::envelope(error)["error"].clone();
    }
}

fn finish(mut checked: CheckedRemoval) -> CheckedRemoval {
    checked.plan["allowed"] = checked.plan["blockers"]
        .as_array()
        .unwrap()
        .is_empty()
        .into();
    checked
}

fn terminal_checks(ps: &Value, row: &Value) -> Value {
    let matches = ps["worktrees"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter(|entry| entry["worktreeId"] == row["id"])
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let observed = (matches.len() == 1).then(|| matches[0]);
    json!({
        "available":observed.is_some(),
        "inventoryTruncated":ps.get("truncated"),
        "hostScope":ps.get("hostScope").map(|scope| json!({"hostIds":scope.get("hostIds"),"omittedHostIds":scope.get("omittedHostIds")})),
        "liveTerminalCount":observed.and_then(|entry| entry["liveTerminalCount"].as_u64()),
        "hasAttachedPty":observed.and_then(|entry| entry["hasAttachedPty"].as_bool()),
        "unverifiableTerminalCount":observed.and_then(|entry| entry["unverifiableTerminalCount"].as_u64()),
        "terminalVerdict":observed.and_then(|entry| entry["terminalVerdict"].as_str()),
        "idle":idle(ps,row).is_ok(),
    })
}

fn terminal_blocker_code(checks: &Value) -> &'static str {
    if checks["liveTerminalCount"]
        .as_u64()
        .is_some_and(|count| count > 0)
        || checks["hasAttachedPty"] == true
    {
        "live_terminals"
    } else if checks["unverifiableTerminalCount"]
        .as_u64()
        .is_some_and(|count| count > 0)
    {
        "terminal_liveness_unknown"
    } else {
        "terminal_inventory_unverified"
    }
}

/// The one shared read-only preflight used by both preview and actual removal.
fn preflight(
    config: &Config,
    selector: &str,
    force: bool,
    run_hooks: bool,
    allow_failed: bool,
) -> CheckedRemoval {
    let mut checked = CheckedRemoval {
        plan: json!({
            "schemaVersion":1,"action":"worktree.remove","dryRun":true,
            "checkedAt":chrono::Utc::now().to_rfc3339(),"requestedSelector":selector,
            "allowed":false,"target":null,"blockers":[],
            "checks":{"git":unavailable_git("Not inspected"),"gitRootVerified":false,
                "terminals":{"available":false,"idle":null},"runtimeConsistent":null,"identityRefreshed":false},
            "effects":{"nativeBranchDeletionPossible":true,"branch":null,
                "runArchiveHooks":run_hooks,"allowFailedArchiveHook":allow_failed,
                "forcePermitsDirtyWorktree":force,"stopsTerminals":false},
            "limitations":["Preflight does not freeze Git state or new terminal activity.",
                "Preview never runs archive hooks or removes a worktree.",
                "Native branch retention and archive-hook guards still apply."]
        }),
        exact: None,
        path: None,
        runtime: None,
    };
    if remove_selector(selector).is_err() {
        blocker(
            &mut checked.plan,
            "explicit_selector_required",
            "Removal requires an explicit worktree selector, not active/current/all",
        );
        return finish(checked);
    }
    if allow_failed && !run_hooks {
        blocker(
            &mut checked.plan,
            "archive_hook_override_requires_hooks",
            "--allow-failed-archive-hook requires --run-hooks",
        );
    }
    if !["ORCA_PAIRING_CODE", "ORCA_ENVIRONMENT"]
        .iter()
        .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()))
    {
        blocker(
            &mut checked.plan,
            "remote_cli_target",
            "Remote CLI targeting is configured; local removal requires a local CLI target",
        );
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    match rpc(config, deadline, "status.get", Value::Null) {
        Ok(status) => {
            checked.runtime = status["runtimeId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            if checked.runtime.is_none() {
                blocker(
                    &mut checked.plan,
                    "runtime_unverified",
                    "Runtime identity is unavailable",
                );
            }
        }
        Err(error) => inspection_blocker(
            &mut checked.plan,
            "runtime_unverified",
            "Runtime identity could not be inspected",
            &error,
        ),
    }
    let shown = match rpc(
        config,
        deadline,
        "worktree.show",
        json!({"worktree":selector}),
    ) {
        Ok(shown) => shown,
        Err(error) => {
            inspection_blocker(
                &mut checked.plan,
                "worktree_unavailable",
                "The selected worktree could not be inspected",
                &error,
            );
            return finish(checked);
        }
    };
    let Some(row) = shown.get("worktree").filter(|row| row.is_object()) else {
        blocker(
            &mut checked.plan,
            "worktree_unavailable",
            "The runtime did not return a worktree",
        );
        return finish(checked);
    };
    checked.plan["target"] = projection(row);
    checked.plan["effects"]["branch"] = row["branch"].as_str().map_or(Value::Null, Value::from);
    if !local(row) {
        blocker(
            &mut checked.plan,
            "host_not_local",
            "Only verified local worktrees can be removed",
        );
    }
    match row["isMainWorktree"].as_bool() {
        Some(false) => (),
        Some(true) => blocker(
            &mut checked.plan,
            "main_worktree",
            "Main worktrees cannot be removed",
        ),
        None => blocker(
            &mut checked.plan,
            "main_status_unknown",
            "Main-worktree status is unknown",
        ),
    }
    match row["isBare"].as_bool() {
        Some(false) => (),
        Some(true) => blocker(
            &mut checked.plan,
            "bare_repository",
            "Bare repositories cannot be removed",
        ),
        None => blocker(
            &mut checked.plan,
            "bare_status_unknown",
            "Bare-repository status is unknown",
        ),
    }
    match exact_selector(row) {
        Ok(exact) => checked.exact = Some(exact),
        Err(error) => inspection_blocker(
            &mut checked.plan,
            "identity_unverified",
            "An exact instance-bound local identity is unavailable",
            &error,
        ),
    }
    match local_path(row) {
        Err(error) => inspection_blocker(
            &mut checked.plan,
            "path_unavailable",
            "The local worktree directory cannot be inspected",
            &error,
        ),
        Ok(path) => {
            checked.plan["target"]["canonicalPath"] = path.to_string_lossy().as_ref().into();
            match safe_git_root(&path, deadline) {
                Ok(()) => checked.plan["checks"]["gitRootVerified"] = true.into(),
                Err(error) => inspection_blocker(
                    &mut checked.plan,
                    "git_root_not_safe",
                    "The target is a base repository or its linked Git root could not be verified",
                    &error,
                ),
            }
            match git_status(&path, deadline) {
                Ok((tracked, untracked)) => {
                    checked.plan["checks"]["git"] = json!({"available":true,"dirty":tracked>0 || untracked>0,"trackedChanges":tracked,"untrackedEntries":untracked,"error":null});
                    if !force && (tracked > 0 || untracked > 0) {
                        blocker(
                            &mut checked.plan,
                            "dirty_worktree",
                            "Worktree has tracked or untracked changes; inspect them or explicitly use --force",
                        );
                    }
                }
                Err(error) => {
                    checked.plan["checks"]["git"] =
                        unavailable_git("Git state could not be verified");
                    inspection_blocker(
                        &mut checked.plan,
                        "git_state_unknown",
                        "Git changes could not be inspected; --force does not waive unknown state",
                        &error,
                    );
                }
            }
            checked.path = Some(path);
        }
    }
    match rpc(config, deadline, "worktree.ps", json!({"limit":10000})) {
        Ok(ps) => {
            let checks = terminal_checks(&ps, row);
            if let Err(error) = idle(&ps, row) {
                inspection_blocker(
                    &mut checked.plan,
                    terminal_blocker_code(&checks),
                    "Worktree has live terminals or its complete idle state could not be verified",
                    &error,
                );
            }
            checked.plan["checks"]["terminals"] = checks;
        }
        Err(error) => inspection_blocker(
            &mut checked.plan,
            "terminal_inventory_unverified",
            "Worktree terminal state could not be inspected",
            &error,
        ),
    }
    if let Some(exact) = &checked.exact {
        match rpc(config, deadline, "worktree.show", json!({"worktree":exact})) {
            Ok(refreshed) => {
                let same = refreshed.get("worktree").is_some_and(|fresh| {
                    protected(fresh).is_ok()
                        && fresh["id"] == row["id"]
                        && fresh["instanceId"] == row["instanceId"]
                        && local_path(fresh).ok() == checked.path
                        && exact_selector(fresh).ok().as_ref() == Some(exact)
                });
                checked.plan["checks"]["identityRefreshed"] = same.into();
                if !same {
                    blocker(
                        &mut checked.plan,
                        "worktree_changed",
                        "Worktree identity, path or protection status changed during inspection",
                    );
                }
            }
            Err(error) => inspection_blocker(
                &mut checked.plan,
                "worktree_changed",
                "The exact worktree identity could not be refreshed",
                &error,
            ),
        }
    }
    match rpc(config, deadline, "status.get", Value::Null) {
        Ok(status) => {
            let same = checked
                .runtime
                .as_deref()
                .is_some_and(|runtime| status["runtimeId"] == runtime);
            checked.plan["checks"]["runtimeConsistent"] = same.into();
            if !same {
                blocker(
                    &mut checked.plan,
                    "runtime_changed",
                    "Runtime identity changed or could not be verified across preflight",
                );
            }
        }
        Err(error) => inspection_blocker(
            &mut checked.plan,
            "runtime_unverified",
            "Runtime identity after preflight could not be inspected",
            &error,
        ),
    }
    finish(checked)
}

/// A nonmutating removal plan. Blockers are data; archive hooks never run here.
pub fn preview(
    config: &Config,
    selector: &str,
    force: bool,
    run_hooks: bool,
    allow_failed: bool,
) -> Result<Value> {
    Ok(preflight(config, selector, force, run_hooks, allow_failed).plan)
}

fn local_inventory_complete(native: &Value) -> bool {
    native["truncated"] == false
        && native
            .pointer("/hostScope/hostIds")
            .and_then(Value::as_array)
            .is_some_and(|hosts| hosts.iter().any(|host| host == "local"))
        && native
            .pointer("/hostScope/omittedHostIds")
            .and_then(Value::as_array)
            .is_some_and(|hosts| !hosts.iter().any(|host| host == "local"))
}

fn absent_instance(native: &Value, instance: &str) -> Option<bool> {
    let rows = native["worktrees"].as_array()?;
    if rows.iter().any(|row| row["instanceId"] == instance) {
        return Some(false);
    }
    if local_inventory_complete(native)
        && rows.iter().all(|row| row["hostId"].as_str().is_some())
        && rows
            .iter()
            .filter(|row| local(row))
            .all(|row| row["instanceId"].as_str().is_some_and(|id| !id.is_empty()))
    {
        Some(true)
    } else {
        None
    }
}

/// Actual removal always performs a fresh preflight; a previous plan is not authority.
/// Native errors propagate unchanged. Acknowledgement without readback is unknown.
pub fn remove(
    config: &Config,
    selector: &str,
    force: bool,
    run_hooks: bool,
    allow_failed: bool,
) -> Result<Value> {
    let checked = preflight(config, selector, force, run_hooks, allow_failed);
    if checked.plan["allowed"] != true {
        let codes = checked.plan["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["code"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(BridgeError::new(
            ErrorCode::PreflightBlocked,
            format!("Removal blocked ({codes}); use --dry-run for the full plan"),
            Outcome::NotStarted,
        )
        .into());
    }
    let exact = checked
        .exact
        .as_deref()
        .context("Removal plan has no exact identity")?;
    let path = checked
        .path
        .as_deref()
        .context("Removal plan has no local path")?;
    let mut args = vec![
        "worktree".into(),
        "rm".into(),
        "--worktree".into(),
        exact.into(),
    ];
    if force {
        args.push("--force".into());
    }
    if run_hooks {
        args.push("--run-hooks".into());
    }
    if allow_failed {
        args.push("--allow-failed-archive-hook".into());
    }
    let operation_id = uuid::Uuid::new_v4().to_string();
    let native = actions::orca(config, &args)?;
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut verification_errors = Vec::new();
    let path_absent = match std::fs::symlink_metadata(path) {
        Ok(_) => Some(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
        Err(_) => {
            verification_errors.push(json!({"code":"path_readback_unavailable","message":"Directory presence could not be inspected after removal"}));
            None
        }
    };
    let instance = checked.plan["target"]["instanceId"]
        .as_str()
        .unwrap_or_default();
    let instance_absent = match rpc(config, deadline, "worktree.list", json!({"limit":10000})) {
        Ok(inventory) => absent_instance(&inventory, instance),
        Err(error) => {
            verification_errors.push(errors::envelope(&error)["error"].clone());
            None
        }
    };
    let runtime_consistent = match rpc(config, deadline, "status.get", Value::Null) {
        Ok(status) => checked
            .runtime
            .as_deref()
            .is_some_and(|runtime| status["runtimeId"] == runtime),
        Err(error) => {
            verification_errors.push(errors::envelope(&error)["error"].clone());
            false
        }
    };
    let verified = path_absent == Some(true) && instance_absent == Some(true) && runtime_consistent;
    Ok(
        json!({"schemaVersion":1,"action":"worktree.remove","operationId":operation_id,
        "completedAt":chrono::Utc::now().to_rfc3339(),"target":checked.plan["target"],
        "acknowledged":true,"verified":verified,"outcome":if verified {"verified"} else {"unknown"},
        "effects":checked.plan["effects"],"nativeResult":native,
        "verification":{"pathAbsent":path_absent,"instanceAbsent":instance_absent,"runtimeConsistent":runtime_consistent,"errors":verification_errors}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    #[test]
    fn allocated_size_parser_checks_units_and_overflow() {
        assert_eq!(du_bytes(b"12\t/fixture path\n").unwrap(), 12288);
        assert_eq!(du_bytes(b"0\t/fixture\n").unwrap(), 0);
        assert!(du_bytes(b"18446744073709551615 /fixture").is_err());
        assert!(du_bytes(b"not-a-size /fixture").is_err());
    }

    #[test]
    fn git_inventory_parser_keeps_nul_paths_and_distinct_detached_locked_prunable_facts() {
        let records=git_worktrees(concat!(
            "worktree /fixture/main\0HEAD abc\0branch refs/heads/main\0\0",
            "worktree /fixture/line\nbreak\0HEAD def\0detached\0locked operator note\nsecond line\0\0",
            "worktree /fixture/missing\0HEAD abc\0branch refs/heads/topic\0prunable gitdir file points to non-existent location\0\0",
            "worktree /fixture/bare\0bare\0\0"
        ).as_bytes()).unwrap();
        assert_eq!(records.len(), 4);
        assert!(records[0].main);
        assert_eq!(records[0].detached, Some(false));
        assert_eq!(records[1].path, PathBuf::from("/fixture/line\nbreak"));
        assert_eq!(records[1].detached, Some(true));
        assert!(records[1].locked);
        assert!(!records[1].prunable);
        assert!(records[2].prunable);
        assert_eq!(records[2].detached, Some(false));
        assert_eq!(records[3].detached, None);
        let discovered = git_discovered_row(
            &records[2],
            &json!({"id":"repo-fixture","externalWorktreeVisibility":"hide"}),
        );
        assert_eq!(discovered["pathExists"], false);
        assert_eq!(discovered["source"], "git-discovered");
        assert!(discovered["worktreeSelector"].is_null());
        assert!(discovered["isArchived"].is_null());
        assert!(git_worktrees(b"worktree /fixture/incomplete").is_err());
        assert!(git_worktrees(b"HEAD abc\0\0").is_err());
        assert!(git_worktrees(b"worktree relative\0\0").is_err());
    }

    #[test]
    fn inventory_unions_only_registered_local_git_and_resolves_hidden_identity_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main");
        let hidden = directory.path().join("hidden-detached");
        for arguments in [
            vec!["init", "--quiet", main.to_str().unwrap()],
            vec![
                "-C",
                main.to_str().unwrap(),
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "--quiet",
                "--message",
                "Fixture",
            ],
            vec![
                "-C",
                main.to_str().unwrap(),
                "worktree",
                "add",
                "--quiet",
                "--detach",
                hidden.to_str().unwrap(),
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let socket_path = directory.path().join("inventory.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::write(directory.path().join("orca-runtime.json"),json!({"runtimeId":"fixture","authToken":"fixture-only-token","transports":[{"kind":"unix","endpoint":socket_path}]}).to_string()).unwrap();
        let main_row = json!({"id":"main-fixture","instanceId":"main-instance","hostId":"local","repoId":"repo-fixture","path":main,"isMainWorktree":true,"isArchived":false});
        let archived_row = json!({"id":"archived-fixture","hostId":"local","repoId":"repo-fixture","path":directory.path().join("archived-missing"),"isMainWorktree":false,"isArchived":true});
        let remote_row = json!({"id":"remote-fixture","hostId":"remote","repoId":"remote-repo","path":main,"isMainWorktree":true,"isArchived":false});
        let repo = json!({"id":"repo-fixture","kind":"git","path":main,"externalWorktreeVisibility":"hide"});
        let hidden_row = json!({"id":"hidden-fixture","instanceId":"hidden-instance","hostId":"local","repoId":"repo-fixture","path":hidden,"isMainWorktree":false,"isArchived":false,"identity":{"executionHostId":"local","instanceId":"hidden-instance","key":"wt2:local:hidden-instance"}});
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            for _ in 0..5 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                methods.push(method.to_owned());
                let result = match method {
                    "worktree.list" => {
                        json!({"worktrees":[main_row,archived_row,remote_row],"truncated":false,"totalCount":3,"hostScope":{"hostIds":["local","remote"],"omittedHostIds":[]}})
                    }
                    "repo.list" => {
                        json!({"repos":[repo,repo,{"id":"remote-repo","kind":"git","path":main,"hostId":"remote"}]})
                    }
                    "worktree.show" => {
                        assert!(
                            request["params"]["worktree"]
                                .as_str()
                                .unwrap()
                                .starts_with("path:")
                        );
                        json!({"worktree":hidden_row})
                    }
                    _ => panic!("unexpected fixture method"),
                };
                writeln!(socket,"{}",json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
            }
            methods
        });
        let config = Config {
            data_dir: directory.path().into(),
            state_dir: directory.path().join("state"),
            orca_cli: directory.path().join("unused-cli"),
            profile_db_override: None,
        };
        let ordinary = inventory(&config, None, 10, Duration::from_secs(2), false).unwrap();
        assert_eq!(ordinary["worktrees"].as_array().unwrap().len(), 3);
        assert_eq!(ordinary["gitDiscoveredCount"], 1);
        assert_eq!(ordinary["excludedExternalCount"], 1);
        let expanded = inventory(&config, None, 10, Duration::from_secs(2), true).unwrap();
        let methods = server.join().unwrap();
        assert_eq!(
            methods,
            vec![
                "worktree.list",
                "repo.list",
                "worktree.list",
                "repo.list",
                "worktree.show"
            ]
        );
        assert_eq!(expanded["worktrees"].as_array().unwrap().len(), 4);
        assert_eq!(expanded["repositoryCount"], 2);
        assert_eq!(expanded["inspectedRepoCount"], 1);
        assert_eq!(expanded["unavailableRepoCount"], 1);
        assert_eq!(expanded["worktrees"][1]["isArchived"], true);
        assert_eq!(expanded["worktrees"][1]["pathExists"], false);
        assert_eq!(expanded["worktrees"][2]["gitStatusAvailable"], false);
        let row = &expanded["worktrees"][3];
        assert_eq!(row["source"], "git-discovered");
        assert_eq!(row["listedByOrca"], false);
        assert_eq!(row["detachedHead"], true);
        assert_eq!(row["pathExists"], true);
        assert_eq!(
            row["worktreeSelector"],
            "identity:wt2:local:hidden-instance"
        );
        let contract: WorktreeInventory = serde_json::from_value(expanded.clone()).unwrap();
        assert_eq!(serde_json::to_value(contract).unwrap(), expanded);
        assert!(hidden.is_dir());
    }

    #[test]
    fn git_counts_rename_paths_once_and_untracked_entries_separately() {
        assert_eq!(
            status_counts(b" M one\0?? dir/\0R  new\0old\0").unwrap(),
            (2, 1)
        );
        assert_eq!(status_counts(b"").unwrap(), (0, 0));
        assert!(status_counts(b"R  new\0").is_err());
        assert!(status_counts(b"bad\0").is_err());
        assert!(status_counts(b" M incomplete").is_err());
    }

    #[test]
    fn projection_cannot_leak_preview_prompt_or_other_native_fields() {
        let row = projection(
            &json!({"id":"fixture","preview":"private","agents":[{"prompt":"private"}],"authToken":"private","displayName":"Fixture"}),
        );
        assert_eq!(row["name"], "Fixture");
        assert!(!row.to_string().contains("private"));
        let row = projection(
            &json!({"hostId":"local","instanceId":"fixture-instance","identity":{"executionHostId":"local","instanceId":"fixture-instance","key":"wt2:local:fixture-instance"}}),
        );
        assert_eq!(
            row["worktreeSelector"],
            "identity:wt2:local:fixture-instance"
        );
    }

    #[test]
    fn removal_rejects_implicit_main_unknown_and_remote_targets() {
        for selector in ["", " ", "active", "current", "all", "*"] {
            assert!(remove_selector(selector).is_err());
        }
        assert!(remove_selector("name:chosen").is_ok());
        for row in [
            json!({"hostId":"local","isMainWorktree":true,"isBare":false}),
            json!({"hostId":"local","isBare":false}),
            json!({"hostId":"remote","isMainWorktree":false,"isBare":false}),
        ] {
            assert!(protected(&row).is_err());
        }
    }

    #[test]
    fn preview_returns_safe_blockers_even_without_a_runtime_and_never_calls_cli() {
        let config = Config {
            data_dir: PathBuf::from("/nonexistent-fixture-worktree-runtime"),
            state_dir: PathBuf::from("/nonexistent-fixture-state"),
            orca_cli: PathBuf::from("/nonexistent-fixture-cli"),
            profile_db_override: None,
        };
        let plan = preview(&config, "current", false, false, false).unwrap();
        assert_eq!(plan["allowed"], false);
        let contract: RemovalPlan = serde_json::from_value(plan.clone()).unwrap();
        assert_eq!(serde_json::to_value(contract).unwrap(), plan);
        assert_eq!(plan["blockers"][0]["code"], "explicit_selector_required");
        let plan = preview(&config, "name:chosen", false, false, true).unwrap();
        assert_eq!(plan["allowed"], false);
        assert!(
            plan["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["code"] == "archive_hook_override_requires_hooks")
        );
        assert!(
            plan["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["code"] == "runtime_unverified")
        );
    }

    #[test]
    fn receipt_readback_requires_complete_inventory_and_preserves_uncertainty() {
        let mut inventory = json!({"truncated":false,"hostScope":{"hostIds":["local"],"omittedHostIds":[]},"worktrees":[]});
        assert_eq!(absent_instance(&inventory, "original"), Some(true));
        inventory["truncated"] = true.into();
        assert_eq!(absent_instance(&inventory, "original"), None);
        inventory["truncated"] = false.into();
        inventory["worktrees"] = json!([{"hostId":"local","instanceId":"original"}]);
        assert_eq!(absent_instance(&inventory, "original"), Some(false));
        inventory["worktrees"] = json!([{"instanceId":"unknown-host"}]);
        assert_eq!(absent_instance(&inventory, "original"), None);
    }

    #[test]
    fn scoped_usage_forwards_repo_and_contract_preserves_unavailable_remote_rows() {
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("usage.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::write(directory.path().join("orca-runtime.json"),json!({"runtimeId":"fixture","authToken":"fixture-only-token","transports":[{"kind":"unix","endpoint":socket_path}]}).to_string()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "worktree.list");
            assert_eq!(request["params"], json!({"limit":1,"repo":"name:fixture"}));
            let result = json!({"worktrees":[{"id":"remote-fixture","hostId":"remote","path":"/remote/fixture","displayName":"Fixture","isMainWorktree":false,"preview":"private fixture"}],"totalCount":2,"truncated":true,"hostScope":{"hostIds":["remote"],"omittedHostIds":["local"]}});
            writeln!(socket,"{}",json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
        });
        let config = Config {
            data_dir: directory.path().into(),
            state_dir: directory.path().join("state"),
            orca_cli: directory.path().join("unused-cli"),
            profile_db_override: None,
        };
        let result = usage_scoped(
            &config,
            None,
            Some("name:fixture"),
            1,
            Duration::from_secs(1),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result["inventoryTruncated"], true);
        assert_eq!(result["measuredCount"], 0);
        assert_eq!(result["worktrees"][0]["disk"]["available"], false);
        assert!(result["worktrees"][0]["worktreeSelector"].is_null());
        assert!(!result.to_string().contains("private fixture"));
        let contract: UsageSnapshot = serde_json::from_value(result.clone()).unwrap();
        assert_eq!(serde_json::to_value(contract).unwrap(), result);
        assert!(
            usage_scoped(
                &config,
                Some("name:chosen"),
                Some("name:fixture"),
                1,
                Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn idle_guard_requires_complete_matching_instance_and_zero_live_processes() {
        let worktree = json!({"id":"tree","instanceId":"instance"});
        let mut ps = json!({"truncated":false,"hostScope":{"hostIds":["local"],"omittedHostIds":[]},"worktrees":[{"worktreeId":"tree","worktreeInstanceId":"instance","hostId":"local","hasAttachedPty":false,"liveTerminalCount":0,"unverifiableTerminalCount":0,"terminalVerdict":"none"}]});
        idle(&ps, &worktree).unwrap();
        ps["worktrees"][0]["hasAttachedPty"] = json!(true);
        assert!(idle(&ps, &worktree).is_err());
        ps["worktrees"][0]["hasAttachedPty"] = json!(false);
        ps["worktrees"][0]["unverifiableTerminalCount"] = json!(1);
        assert!(idle(&ps, &worktree).is_err());
        ps["worktrees"][0]["unverifiableTerminalCount"] = json!(0);
        ps["truncated"] = json!(true);
        assert!(idle(&ps, &worktree).is_err());
    }

    #[test]
    fn subprocess_capture_bounds_output_and_timeout_without_waiting_on_grandchildren() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '12345'"]);
        assert_eq!(
            command_output(command, Instant::now() + Duration::from_secs(1), 5).unwrap(),
            b"12345"
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '12345'"]);
        assert!(
            command_output(command, Instant::now() + Duration::from_secs(1), 4)
                .unwrap_err()
                .downcast_ref::<InspectionLimit>()
                .is_some()
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 20 & wait"]);
        let start = Instant::now();
        assert!(command_output(command, start + Duration::from_millis(150), 1024).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 20 & exit 0"]);
        let start = Instant::now();
        assert!(command_output(command, start + Duration::from_secs(1), 1024).is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn preview_is_nonmutating_and_remove_rechecks_before_fake_cli_delegation() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main");
        let tree = directory.path().join("chosen");
        let commands = [
            vec!["init", "--quiet", main.to_str().unwrap()],
            vec![
                "-C",
                main.to_str().unwrap(),
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "--quiet",
                "--message",
                "Fixture",
            ],
            vec![
                "-C",
                main.to_str().unwrap(),
                "worktree",
                "add",
                "--quiet",
                "-b",
                "fixture-cleanup",
                tree.to_str().unwrap(),
            ],
        ];
        for arguments in commands {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(tree.join("draft.txt"), "synthetic untracked work").unwrap();
        let cli = directory.path().join("fake-orca");
        std::fs::write(&cli, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ORCA_USER_DATA_PATH/delegated-argv\"\nprintf '{\"ok\":true,\"result\":{\"fixture\":true}}\\n'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket_path = directory.path().join("runtime.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::write(directory.path().join("orca-runtime.json"), json!({"runtimeId":"fixture","authToken":"fixture-only-token","transports":[{"kind":"unix","endpoint":socket_path}]}).to_string()).unwrap();
        let row = json!({"id":"fixture-tree","instanceId":"fixture-instance","hostId":"local",
            "identity":{"key":"wt2:local:fixture-instance","instanceId":"fixture-instance","executionHostId":"local"},
            "path":tree,"isMainWorktree":false,"isBare":false});
        let finished = Arc::new(AtomicBool::new(false));
        let server_finished = finished.clone();
        let scenario = Arc::new(AtomicUsize::new(0));
        let server_scenario = scenario.clone();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            while Instant::now() < deadline && !server_finished.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture socket failed: {error}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "status.get" => json!({"runtimeId":"fixture"}),
                    "worktree.show" => {
                        let mut current = row.clone();
                        if server_scenario.load(Ordering::Relaxed) == 2 {
                            current["isMainWorktree"] = true.into();
                        }
                        json!({"worktree":current})
                    }
                    "worktree.list" => {
                        json!({"worktrees":[row],"truncated":false,"hostScope":{"hostIds":["local"],"omittedHostIds":[]}})
                    }
                    "worktree.ps" => {
                        let live = server_scenario.load(Ordering::Relaxed) == 1;
                        json!({"truncated":false,"hostScope":{"hostIds":["local"],"omittedHostIds":[]},"worktrees":[{"worktreeId":"fixture-tree","worktreeInstanceId":"fixture-instance","hostId":"local","hasAttachedPty":live,"liveTerminalCount":if live {1} else {0},"unverifiableTerminalCount":0,"terminalVerdict":if live {"live"} else {"none"}}]})
                    }
                    _ => panic!("Unexpected fixture RPC"),
                };
                writeln!(socket,"{}",json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
                requests.push(request);
            }
            requests
        });
        let config = Config {
            data_dir: directory.path().into(),
            state_dir: directory.path().join("state"),
            orca_cli: cli,
            profile_db_override: None,
        };
        let plan = preview(&config, "name:chosen", false, true, true).unwrap();
        assert_eq!(plan["allowed"], false);
        assert!(
            plan["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["code"] == "dirty_worktree")
        );
        assert!(!directory.path().join("delegated-argv").exists());
        let plan = preview(&config, "name:chosen", true, true, true).unwrap();
        assert_eq!(plan["allowed"], true);
        assert_eq!(
            plan["target"]["worktreeSelector"],
            "identity:wt2:local:fixture-instance"
        );
        assert_eq!(plan["effects"]["runArchiveHooks"], true);
        assert!(!directory.path().join("delegated-argv").exists());
        scenario.store(1, Ordering::Relaxed);
        let error = remove(&config, "name:chosen", true, true, true).unwrap_err();
        assert_eq!(
            errors::envelope(&error)["error"]["code"],
            "preflight_blocked"
        );
        assert!(!directory.path().join("delegated-argv").exists());
        scenario.store(2, Ordering::Relaxed);
        let plan = preview(&config, "name:chosen", true, true, true).unwrap();
        assert_eq!(plan["allowed"], false);
        assert!(
            plan["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["code"] == "main_worktree")
        );
        assert!(!directory.path().join("delegated-argv").exists());
        scenario.store(0, Ordering::Relaxed);
        let result = remove(&config, "name:chosen", true, true, true);
        finished.store(true, Ordering::Relaxed);
        let requests = server.join().unwrap();
        let receipt = result.unwrap();
        let contract: RemovalReceipt = serde_json::from_value(receipt.clone()).unwrap();
        assert_eq!(serde_json::to_value(contract).unwrap(), receipt);
        assert_eq!(receipt["nativeResult"], json!({"fixture":true}));
        assert_eq!(receipt["acknowledged"], true);
        assert_eq!(receipt["verified"], false);
        assert_eq!(receipt["outcome"], "unknown");
        assert_eq!(receipt["verification"]["pathAbsent"], false);
        assert_eq!(requests.len(), 27);
        let delegated = std::fs::read_to_string(directory.path().join("delegated-argv")).unwrap();
        assert_eq!(
            delegated,
            "worktree\nrm\n--worktree\nidentity:wt2:local:fixture-instance\n--force\n--run-hooks\n--allow-failed-archive-hook\n--json\n"
        );
        assert!(tree.join("draft.txt").is_file());
    }
}
