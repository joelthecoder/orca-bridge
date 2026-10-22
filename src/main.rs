use anyhow::{Context, Result, bail, ensure};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use orca_bridge::{
    actions, config::Config, events, history, model::Session, projects, rpc::RpcClient, skills,
    stream, terminal, ui,
};
use serde_json::{Value, json};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    version,
    about = "Find the session. Keep the flow.\nSearch Orca history, navigate conversations, automate workspaces and follow activity.",
    styles = terminal::styles(),
    disable_help_subcommand = true,
    after_help = "QUICK START\n  orca-bridge search 'migration'\n  orca-bridge recent --limit 10\n  orca-bridge terminals list\n  orca-bridge watch --topics sessions,status --timeout 5\n\nFOR AGENTS\n  orca-bridge help --json       Complete machine-readable command tree\n  orca-bridge agent             Portable usage guide and automation contracts\n\nPipes default to JSON. Streams emit JSONL. Diagnostics go to stderr.\nUse --help on any command, or `orca -- <args>` for the installed Orca CLI."
)]
struct Cli {
    #[arg(long, global=true, conflicts_with_all=["jsonl", "alfred"])]
    /// Emit one machine-readable JSON result
    json: bool,
    #[arg(long, global = true, conflicts_with = "alfred")]
    /// Emit list entries individually as JSON Lines
    jsonl: bool,
    #[arg(long, global = true)]
    /// Emit Alfred Script Filter items for session lists
    alfred: bool,
    /// Human-readable output, including when redirected
    #[arg(long, global=true, conflicts_with_all=["json", "jsonl", "alfred"])]
    human: bool,
    /// Terminal colors; structured output is always unstyled
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: clap::ColorChoice,
    #[arg(long, global = true)]
    /// Orca data directory (also ORCA_USER_DATA_PATH)
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    /// Private bridge state directory (also ORCA_BRIDGE_STATE_DIR)
    state_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    /// Explicit Orca profile-state.db; otherwise follow the active profile
    profile_db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the bundled assistant skill, or explicitly install it for Claude/Codex
    #[command(alias = "skills")]
    Skill {
        #[command(subcommand)]
        command: Option<SkillCommand>,
    },
    /// Export a versioned UI snapshot of history, projects, repositories, worktrees and live tabs
    Snapshot {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Include project artwork data; omitted by default to keep snapshots compact
        #[arg(long)]
        icons: bool,
    },
    /// Print generated JSON Schema for a data type, without Orca access
    Schema {
        #[arg(value_enum)]
        entity: SchemaEntity,
    },
    /// Read command help; --json exposes the full command tree without runtime access
    Help { path: Vec<String> },
    /// Print the portable automation guide, examples and output contracts
    Agent,
    /// Manage Orca's built-in scheduled automations and run history
    #[command(
        after_help = "EXAMPLES\n  orca-bridge automations list --json\n  orca-bridge automations create --name 'Daily review' --trigger daily --time 09:00 --timezone America/Edmonton --repo name:project --agent codex --prompt - --disabled\n  orca-bridge automations edit ID --enabled\n  orca-bridge automations run ID\n\nSchedules and ownership are managed by Orca, using its installed version's validation."
    )]
    Automations {
        #[command(subcommand)]
        command: AutomationCommand,
    },
    /// Search and manage recorded agent sessions
    #[command(alias = "session")]
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Fuzzy search session titles, paths, branches, agents and models
    Search(Search),
    /// List recently updated sessions
    Recent(List),
    /// Query registered projects or add a project
    #[command(
        after_help = "EXAMPLES\n  orca-bridge projects list --json\n  orca-bridge projects settings show name:project --json\n  orca-bridge projects settings set name:project --updates '{\"displayName\":\"My Project\"}'\n  orca-bridge projects icon apply name:project --file icon.png"
    )]
    Projects {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Inspect or create worktrees
    Worktrees {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    /// Inspect, control and follow live terminals
    #[command(
        after_help = "EXAMPLES\n  orca-bridge terminals list --json\n  orca-bridge terminals follow HANDLE --timeout 30\n  printf 'Review the diff' | orca-bridge terminals send HANDLE --text - --enter\n\nDiscover handles with list; reacquire them after a runtime/incarnation change."
    )]
    Terminals {
        #[command(subcommand)]
        command: TerminalCommand,
    },
    /// Stream and replay observed activity
    Events {
        #[command(subcommand)]
        command: EventCommand,
    },
    /// Stream observed state changes as JSONL (alias for events watch)
    Watch(Watch),
    /// Enroll or use Orca's native encrypted subscription transport
    Stream {
        #[command(subcommand)]
        command: StreamCommand,
    },
    /// Call a local Orca RPC, or process a JSONL request pipe
    Rpc(Rpc),
    /// Filter arbitrary JSONL from stdin with fuzzy matching
    Filter {
        query: String,
        #[arg(long)]
        /// JSON Pointer to the field to match, such as /title
        field: Option<String>,
    },
    /// Open a session ID or orca-bridge://session link
    Open { target: String },
    /// Inspect local compatibility and tool health
    Doctor,
    /// Run any installed Orca command with structured arguments
    Orca {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage the local activity recorder
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Args, Clone)]
struct List {
    #[arg(short, long, default_value_t = 10)]
    /// Maximum number of returned entries
    limit: usize,
    #[arg(long)]
    /// Agent/provider ID, such as codex or claude
    agent: Option<String>,
    #[arg(long)]
    path: Option<String>,
    /// ISO 8601 lower bound for session update time
    #[arg(long)]
    since: Option<String>,
}
#[derive(Args)]
#[command(
    after_help = "EXAMPLES\n  orca-bridge search 'migration' --agent codex\n  orca-bridge search 'retry budget' --full-text --json\n  orca-bridge recent --jsonl | orca-bridge search rust --stdin --jsonl\n\nMetadata search reads the local index; no running app or pairing is needed."
)]
struct Search {
    #[arg(default_value = "")]
    query: String,
    #[command(flatten)]
    list: List,
    /// Search indexed conversation content using Orca
    #[arg(long)]
    full_text: bool,
    /// Read Session JSONL from stdin instead of Orca's database
    #[arg(long)]
    stdin: bool,
}
#[derive(Subcommand)]
enum SessionCommand {
    /// Rename the bridge display title; empty resets it. Provider history is preserved
    Rename {
        id: String,
        title: String,
    },
    /// Stop a proven live session's exact PTY, preserving conversation history
    Stop {
        id: String,
    },
    Recent(List),
    Search(Search),
    Show {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
        #[arg(long)]
        /// Include bounded provider usage counters
        usage: bool,
    },
    Usage {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
    },
    Open {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
    },
    /// Explicitly resume history in a new terminal
    Resume {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
    },
    /// Print a reusable session link
    Link {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
    },
    /// Stream the source transcript without rewriting its records
    Transcript {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
        #[arg(short, long)]
        /// Keep following appended transcript records
        follow: bool,
        #[arg(long)]
        /// Start at the end of the transcript
        tail: bool,
        #[arg(long)]
        /// Stop after this many data records
        count: Option<usize>,
        #[arg(long)]
        /// Stop after this many seconds
        timeout: Option<u64>,
    },
    /// Show terminals with explicit attention or waiting status
    Attention,
    Create {
        #[arg(long)]
        /// Orca worktree selector, such as path:/absolute/project
        worktree: Option<String>,
        #[arg(long)]
        /// Durable Orca project ID
        project: Option<String>,
        #[arg(long)]
        /// Display name for the new or updated item
        name: Option<String>,
        #[arg(long, default_value = "codex")]
        /// Agent/provider ID, such as codex or claude
        agent: String,
        #[arg(long)]
        /// Provider model ID for the new agent session
        model: Option<String>,
        #[arg(long)]
        /// Reasoning effort for the selected model
        effort: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        /// Reveal the created session or terminal in Orca
        focus: bool,
    },
    /// Send literal text to a live terminal; '-' consumes stdin
    Send {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
        #[arg(long, default_value = "-")]
        /// Literal input text; use - to read stdin
        text: String,
        #[arg(long)]
        /// Submit Enter after the literal text
        enter: bool,
        #[arg(long)]
        /// Request interrupt-style input when supported by Orca
        interrupt: bool,
    },
}
#[derive(Args)]
struct AutomationTarget {
    /// Registered repository selector for a new worktree per run
    #[arg(long, conflicts_with = "worktree")]
    repo: Option<String>,
    /// Existing workspace selector; forwarded to Orca as --workspace
    #[arg(long, conflicts_with = "repo")]
    worktree: Option<String>,
}
#[derive(Args)]
struct AutomationTiming {
    /// HH:MM local time for daily, weekdays or weekly presets
    #[arg(long)]
    time: Option<String>,
    /// Weekly preset day, Sunday=0
    #[arg(long, value_parser=clap::value_parser!(u8).range(0..=6))]
    day: Option<u8>,
    /// IANA time zone, such as America/Edmonton
    #[arg(long)]
    timezone: Option<String>,
}
#[derive(Args)]
struct AutomationEnabled {
    /// Enable scheduled runs
    #[arg(long, conflicts_with = "disabled")]
    enabled: bool,
    /// Save the automation without scheduled execution
    #[arg(long, conflicts_with = "enabled")]
    disabled: bool,
}
#[derive(Subcommand)]
enum AutomationCommand {
    /// List Orca's scheduled automations
    List,
    /// Inspect one automation and its schedule
    Show { id: String },
    /// Create an Orca automation; --disabled lets you review it before enabling
    Create {
        #[arg(long)]
        /// Display name for the new or updated item
        name: String,
        /// hourly, daily, weekdays, weekly, five-field cron, or RRULE
        #[arg(long)]
        trigger: String,
        #[arg(long, alias = "provider")]
        /// Agent/provider ID, such as codex or claude
        agent: String,
        /// Literal prompt text; '-' reads stdin
        #[arg(long)]
        prompt: String,
        #[command(flatten)]
        /// Durable session ID or orca-bridge://session URL
        target: AutomationTarget,
        #[command(flatten)]
        timing: AutomationTiming,
        #[command(flatten)]
        enabled: AutomationEnabled,
    },
    /// Patch only supplied automation fields, using Orca's owner fencing
    Edit {
        /// Durable item ID; session commands also accept unique session prefixes
        id: String,
        #[arg(long)]
        /// Display name for the new or updated item
        name: Option<String>,
        #[arg(long)]
        trigger: Option<String>,
        #[arg(long, alias = "provider")]
        /// Agent/provider ID, such as codex or claude
        agent: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[command(flatten)]
        /// Durable session ID or orca-bridge://session URL
        target: AutomationTarget,
        #[command(flatten)]
        timing: AutomationTiming,
        #[command(flatten)]
        enabled: AutomationEnabled,
    },
    /// Run an automation now, using its configured agent and workspace
    Run { id: String },
    /// List run history for one automation, or all when omitted
    Runs {
        #[arg(long)]
        id: Option<String>,
    },
    /// Delete an automation and its run history through Orca
    Remove { id: String },
}

#[derive(Subcommand)]
enum ProjectCommand {
    List,
    Add {
        path: PathBuf,
    },
    /// Read or patch repository settings through the local runtime
    Settings {
        #[command(subcommand)]
        command: ProjectSettingsCommand,
    },
    /// Store reusable repository artwork and apply a validated PNG
    Icon {
        #[command(subcommand)]
        command: ProjectIconCommand,
    },
}
#[derive(Subcommand)]
enum ProjectSettingsCommand {
    Show {
        /// Repository selector, such as name:project or path:/absolute/project
        repo: String,
    },
    Set {
        /// Repository selector, such as name:project or path:/absolute/project
        repo: String,
        #[arg(long, default_value = "{}", conflicts_with = "stdin")]
        /// JSON object containing only the settings to change
        updates: String,
        #[arg(long)]
        /// Read the command input from stdin
        stdin: bool,
    },
}
#[derive(Subcommand)]
enum ProjectIconCommand {
    /// Copy a square PNG into Assets/Orca/ProjectIcon without replacing existing art
    Import {
        /// Repository selector, such as name:project or path:/absolute/project
        repo: String,
        #[arg(long)]
        /// Local PNG artwork file
        file: PathBuf,
    },
    /// Apply a square PNG (maximum 256 KiB), preserving all other settings
    Apply {
        /// Repository selector, such as name:project or path:/absolute/project
        repo: String,
        #[arg(long)]
        /// Local PNG artwork file
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum WorktreeCommand {
    List {
        #[arg(long)]
        /// Durable Orca project ID
        project: Option<String>,
        #[arg(long)]
        /// Repository selector, such as name:project or path:/absolute/project
        repo: Option<String>,
    },
    Show {
        /// Orca worktree selector
        selector: String,
    },
    Create {
        #[arg(long)]
        /// Repository selector, such as name:project or path:/absolute/project
        repo: Option<String>,
        #[arg(long)]
        /// Durable Orca project ID
        project: Option<String>,
        #[arg(long)]
        /// Display name for the new or updated item
        name: String,
        #[arg(long)]
        /// Agent/provider ID, such as codex or claude
        agent: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        activate: bool,
    },
}
#[derive(Subcommand)]
enum TerminalCommand {
    /// Set the live tab title, or pass an empty title to reset it
    Rename { terminal: String, title: String },
    /// Stop this exact live PTY, preserving recorded conversation history
    Stop { terminal: String },
    /// Close a terminal pane, or its entire tab with --tab
    Close {
        terminal: String,
        #[arg(long)]
        tab: bool,
    },
    List {
        #[arg(long)]
        /// Orca worktree selector, such as path:/absolute/project
        worktree: Option<String>,
        #[arg(long)]
        /// Include terminal tab and pane layout metadata
        layouts: bool,
    },
    Show {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
    },
    Read {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
        #[arg(long)]
        /// Output cursor returned by a previous terminal read
        cursor: Option<u64>,
        #[arg(long, default_value_t = 65536)]
        /// Maximum number of returned entries
        limit: usize,
    },
    Follow {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
        #[arg(long)]
        /// Output cursor returned by a previous terminal read
        cursor: Option<u64>,
        #[arg(long, default_value_t = 250)]
        /// Polling interval in milliseconds, minimum 100
        interval_ms: u64,
        #[arg(long)]
        /// Stop after this many data records
        count: Option<usize>,
        #[arg(long)]
        /// Stop after this many seconds
        timeout: Option<u64>,
    },
    Focus {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
    },
    Send {
        /// Live runtime-issued terminal handle; discover with terminals list
        terminal: String,
        #[arg(long, default_value = "-")]
        /// Literal input text; use - to read stdin
        text: String,
        #[arg(long)]
        /// Submit Enter after the literal text
        enter: bool,
        #[arg(long)]
        /// Request interrupt-style input when supported by Orca
        interrupt: bool,
    },
    Create {
        #[arg(long)]
        /// Orca worktree selector, such as path:/absolute/project
        worktree: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        /// Reveal the created session or terminal in Orca
        focus: bool,
    },
}
#[derive(Args)]
struct Watch {
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "sessions,terminals,status,notifications,worktrees,projects"
    )]
    /// Comma-separated observed topics
    topics: Vec<String>,
    #[arg(long, default_value_t = 1000)]
    /// Polling interval in milliseconds, minimum 100
    interval_ms: u64,
    #[arg(long)]
    /// Omit initial state snapshots
    no_initial: bool,
    #[arg(long)]
    /// Record observed events in the private bridge journal
    record: bool,
    /// Journal retention when recording; enforced at startup and every minute
    #[arg(long, default_value_t = 30)]
    retention_days: u32,
    #[arg(long, default_value_t = 100000)]
    retention_count: usize,
    #[arg(long)]
    /// Stop after this many data records
    count: Option<usize>,
    #[arg(long)]
    /// Stop after this many seconds
    timeout: Option<u64>,
}
#[derive(Subcommand)]
enum EventCommand {
    Watch(Watch),
    /// Report available replay cursor bounds
    Status,
    /// Remove old journal events, preserving monotonic replay IDs
    Prune {
        #[arg(long, default_value_t = 30)]
        /// Maximum journal age in days
        keep_days: u32,
        #[arg(long, default_value_t = 100000)]
        /// Maximum retained journal events
        keep_count: usize,
    },
    List {
        #[arg(long, default_value_t = 0)]
        /// Replay only events after this persistent local cursor
        since_id: i64,
        #[arg(long, default_value_t = 100)]
        /// Maximum number of returned entries
        limit: usize,
        #[arg(long)]
        /// Filter replay by one event topic
        topic: Option<String>,
    },
    /// Replay a durable cursor then keep following recorded events
    Follow {
        #[arg(long, default_value_t = 0)]
        /// Replay only events after this persistent local cursor
        since_id: i64,
        #[arg(long)]
        /// Filter replay by one event topic
        topic: Option<String>,
        #[arg(long)]
        /// Stop after this many data records
        count: Option<usize>,
        #[arg(long)]
        /// Stop after this many seconds
        timeout: Option<u64>,
    },
}
#[derive(Subcommand)]
enum StreamCommand {
    /// Import and verify a runtime pairing URL from stdin
    Pair {
        #[arg(long, required = true)]
        /// Read the command input from stdin
        stdin: bool,
    },
    Status,
    /// Native push subscription; reconnects after transport failures
    Subscribe {
        /// Orca private RPC method name
        method: String,
        #[arg(long, default_value = "null")]
        /// JSON parameters for the selected runtime method
        params: String,
        #[arg(long)]
        /// Stop after this many data records
        count: Option<usize>,
        #[arg(long)]
        /// Stop after this many seconds
        timeout: Option<u64>,
        #[arg(long)]
        /// Fail on transport loss instead of reconnecting
        no_reconnect: bool,
    },
}
#[derive(Args)]
struct Rpc {
    /// Orca private RPC method name
    method: Option<String>,
    #[arg(long, default_value = "null")]
    /// JSON parameters for the selected runtime method
    params: String,
    /// Read one JSON params object from stdin
    #[arg(long, conflicts_with = "pipe")]
    stdin: bool,
    /// Process {id,method,params} JSONL; emit one correlated result per input
    #[arg(long, conflicts_with = "method")]
    pipe: bool,
}
#[derive(Subcommand, Clone, Copy)]
enum ServiceCommand {
    Install,
    Status,
    Uninstall,
}
#[derive(Clone, Copy, ValueEnum)]
enum SchemaEntity {
    Session,
    SessionView,
    Snapshot,
}
#[derive(Subcommand)]
enum SkillCommand {
    /// Install the bundled skill, preserving existing different skills and symlinks
    Install {
        #[arg(long, conflicts_with = "codex", required_unless_present = "codex")]
        claude: bool,
        #[arg(long, conflicts_with = "claude", required_unless_present = "claude")]
        codex: bool,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Jsonl,
    Alfred,
}

fn emit(value: &Value, format: Format, out: &mut impl Write) -> Result<()> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *out, value)?;
            out.write_all(b"\n")?;
        }
        Format::Jsonl => {
            if let Some(items) = value.as_array() {
                for item in items {
                    line(item, out)?;
                }
            } else {
                line(value, out)?;
            }
        }
        Format::Alfred => {
            let items=value.as_array().context("Alfred output requires a list")?.iter().map(|s|json!({"uid":s["id"],"title":s["title"],"subtitle":format!("{} · {} · {}",s["agent"].as_str().unwrap_or(""),s["model"].as_str().unwrap_or("unknown model"),s["cwd"].as_str().unwrap_or("")),"arg":s["id"],"valid":true})).collect::<Vec<_>>();
            line(&json!({"items":items}), out)?;
        }
        Format::Text => {
            if let Some(items) = value.as_array() {
                for s in items {
                    if s.get("title").is_some() {
                        write!(out, "{}", terminal::session(s))?;
                    } else {
                        line(s, out)?;
                    }
                }
            } else {
                serde_json::to_writer_pretty(&mut *out, value)?;
                out.write_all(b"\n")?;
            }
        }
    }
    out.flush()?;
    Ok(())
}
fn line(value: &Value, out: &mut impl Write) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
const MAX_RECORD: usize = 8 * 1024 * 1024;

fn read_record(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let n = (&mut *reader)
        .take((MAX_RECORD + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    ensure!(n <= MAX_RECORD, "Input record exceeds 8 MiB");
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(bytes).context("Input record is not UTF-8")?,
    ))
}

fn stdin_text() -> Result<String> {
    ensure!(
        !io::stdin().is_terminal(),
        "Pipe text into stdin, or provide --text/--prompt"
    );
    let mut text = String::new();
    io::stdin()
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    ensure!(text.len() <= 8 * 1024 * 1024, "stdin exceeds 8 MiB");
    Ok(text)
}
fn literal(text: String) -> Result<String> {
    if text == "-" { stdin_text() } else { Ok(text) }
}
fn filters(mut sessions: Vec<Session>, list: &List) -> Result<Vec<Session>> {
    let since = list
        .since
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .context("--since must be ISO 8601 with a timezone")?;
    sessions.retain(|s| {
        list.agent.as_ref().is_none_or(|a| s.agent == *a)
            && list
                .path
                .as_ref()
                .is_none_or(|p| s.cwd.as_ref().is_some_and(|c| c.starts_with(p)))
            && since.is_none_or(|d| {
                s.updated_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .is_some_and(|t| t >= d)
            })
    });
    Ok(sessions)
}
fn search(config: &Config, s: Search) -> Result<Value> {
    if s.full_text {
        ensure!(!s.stdin, "--stdin cannot be used with --full-text");
        ensure!(!s.query.is_empty(), "Full-text search needs a query");
        let mut args = vec![
            "search".into(),
            s.query,
            "--limit".into(),
            s.list.limit.to_string(),
        ];
        for (flag, value) in [
            ("--agent", s.list.agent),
            ("--path", s.list.path),
            ("--since", s.list.since),
        ] {
            if let Some(value) = value {
                args.extend([flag.into(), value]);
            }
        }
        return actions::orca(config, &args);
    }
    let sessions = if s.stdin {
        let input = stdin_text()?;
        if input.trim_start().starts_with('[') {
            serde_json::from_str(&input)?
        } else {
            input
                .lines()
                .filter(|s| !s.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<std::result::Result<_, _>>()?
        }
    } else {
        history::load_sessions(config)?
    };
    Ok(serde_json::to_value(history::search(
        filters(sessions, &s.list)?,
        &s.query,
        s.list.limit,
    ))?)
}
fn send(
    config: &Config,
    terminal: String,
    text: String,
    enter: bool,
    interrupt: bool,
) -> Result<Value> {
    let mut args = vec![
        "terminal".into(),
        "send".into(),
        "--terminal".into(),
        terminal,
        "--text".into(),
        literal(text)?,
    ];
    if enter {
        args.push("--enter".into());
    }
    if interrupt {
        args.push("--interrupt".into());
    }
    actions::orca(config, &args)
}
fn automation(config: &Config, command: AutomationCommand) -> Result<Value> {
    let mut args = vec!["automations".into()];
    fn options(
        args: &mut Vec<String>,
        target: AutomationTarget,
        timing: AutomationTiming,
        enabled: AutomationEnabled,
    ) {
        optional_arg(args, "--repo", target.repo);
        optional_arg(args, "--workspace", target.worktree);
        optional_arg(args, "--time", timing.time);
        optional_arg(args, "--day", timing.day.map(|d| d.to_string()));
        optional_arg(args, "--timezone", timing.timezone);
        if enabled.enabled {
            args.push("--enabled".into());
        }
        if enabled.disabled {
            args.push("--disabled".into());
        }
    }
    match command {
        AutomationCommand::List => args.push("list".into()),
        AutomationCommand::Show { id } => args.extend(["show".into(), "--id".into(), id]),
        AutomationCommand::Run { id } => args.extend(["run".into(), "--id".into(), id]),
        AutomationCommand::Remove { id } => args.extend(["remove".into(), "--id".into(), id]),
        AutomationCommand::Runs { id } => {
            args.push("runs".into());
            optional_arg(&mut args, "--id", id);
        }
        AutomationCommand::Create {
            name,
            trigger,
            agent,
            prompt,
            target,
            timing,
            enabled,
        } => {
            args.extend([
                "create".into(),
                "--name".into(),
                name,
                "--trigger".into(),
                trigger,
                "--provider".into(),
                agent,
                "--prompt".into(),
                literal(prompt)?,
            ]);
            options(&mut args, target, timing, enabled);
        }
        AutomationCommand::Edit {
            id,
            name,
            trigger,
            agent,
            prompt,
            target,
            timing,
            enabled,
        } => {
            args.extend(["edit".into(), "--id".into(), id]);
            optional_arg(&mut args, "--name", name);
            optional_arg(&mut args, "--trigger", trigger);
            optional_arg(&mut args, "--provider", agent);
            optional_arg(&mut args, "--prompt", prompt.map(literal).transpose()?);
            options(&mut args, target, timing, enabled);
        }
    }
    actions::orca(config, &args)
}

fn optional_arg(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(v) = value {
        args.extend([flag.into(), v]);
    }
}
fn run_session(
    config: &Config,
    command: SessionCommand,
    format: Format,
    out: &mut impl Write,
) -> Result<()> {
    let value = match command {
        SessionCommand::Rename { id, title } => {
            serde_json::to_value(history::rename(config, &id, &title)?)?
        }
        SessionCommand::Stop { id } => {
            actions::stop_session(config, &history::get_session(config, &id)?)?
        }
        SessionCommand::Recent(list) => serde_json::to_value(history::recent(
            filters(history::load_sessions(config)?, &list)?,
            list.limit,
        ))?,
        SessionCommand::Search(s) => search(config, s)?,
        SessionCommand::Show { id, usage } => {
            let mut s = history::get_session(config, &id)?;
            if usage {
                s.usage = Some(history::usage(&s)?);
            }
            serde_json::to_value(s)?
        }
        SessionCommand::Usage { id } => history::usage(&history::get_session(config, &id)?)?,
        SessionCommand::Open { id } => {
            actions::open_session(config, &history::get_session(config, &id)?)?
        }
        SessionCommand::Resume { id } => {
            actions::resume_session(config, &history::get_session(config, &id)?)?
        }
        SessionCommand::Link { id } => {
            let s = history::get_session(config, &id)?;
            let link = actions::link(&s)?;
            if matches!(format, Format::Text) {
                writeln!(out, "{link}")?;
                return Ok(());
            }
            json!({"id":s.id,"url":link})
        }
        SessionCommand::Transcript {
            id,
            follow,
            tail,
            count,
            timeout,
        } => return transcript(config, &id, follow, tail, count, timeout, out),
        SessionCommand::Attention => {
            let rpc = RpcClient::new(config);
            let t = rpc.call("terminal.list", json!({"includeVisualLayouts":false}))?;
            let mut rows = Vec::new();
            for t in t["terminals"].as_array().context("Missing terminals")? {
                let status = rpc.call(
                    "terminal.agentStatus",
                    json!({"terminal":t["handle"],"expectedIncarnationId":t["incarnationId"]}),
                )?;
                let state = status
                    .pointer("/agentStatus/status")
                    .and_then(Value::as_str);
                if state.is_some_and(|s| {
                    matches!(s, "attention" | "waiting" | "blocked" | "needs-input")
                }) {
                    rows.push(json!({"terminal":t,"status":status["agentStatus"]}));
                }
            }
            json!(rows)
        }
        SessionCommand::Send {
            terminal,
            text,
            enter,
            interrupt,
        } => send(config, terminal, text, enter, interrupt)?,
        SessionCommand::Create {
            worktree,
            project,
            name,
            agent,
            model,
            effort,
            prompt,
            focus,
        } => {
            let mut args = if let Some(project) = project {
                vec![
                    "worktree".into(),
                    "create".into(),
                    "--project".into(),
                    project,
                    "--name".into(),
                    name.context("--project requires --name for the new worktree")?,
                ]
            } else {
                ensure!(
                    name.is_none(),
                    "Use --project with --name, or --worktree for an existing workspace"
                );
                vec!["agent".into(), "launch".into()]
            };
            args.extend(["--agent".into(), agent]);
            optional_arg(&mut args, "--worktree", worktree);
            optional_arg(&mut args, "--model", model);
            optional_arg(&mut args, "--effort", effort);
            optional_arg(&mut args, "--prompt", prompt.map(literal).transpose()?);
            if focus {
                args.push(
                    if args[0] == "worktree" {
                        "--activate"
                    } else {
                        "--focus"
                    }
                    .into(),
                );
            }
            actions::orca(config, &args)?
        }
    };
    emit(&value, format, out)
}
fn run_watch(config: &Config, w: Watch, out: &mut impl Write) -> Result<()> {
    ensure!(w.count != Some(0), "--count must be positive");
    events::watch(
        config,
        events::WatchOptions {
            topics: w.topics,
            interval: Duration::from_millis(w.interval_ms),
            timeout: w.timeout.map(Duration::from_secs),
            count: w.count,
            initial: !w.no_initial,
            record: w.record,
            retention_days: w.retention_days,
            retention_count: w.retention_count,
        },
        out,
    )
}
fn transcript(
    config: &Config,
    id: &str,
    follow: bool,
    tail: bool,
    count: Option<usize>,
    timeout: Option<u64>,
    out: &mut impl Write,
) -> Result<()> {
    let session = history::get_session(config, id)?;
    let path = PathBuf::from(
        session
            .file_path
            .context("This session has no local transcript file")?,
    );
    ensure!(path.is_file(), "Transcript is not a local file");
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(&path)?;
    let mut offset = if tail { file.metadata()?.len() } else { 0 };
    let start = Instant::now();
    let mut emitted = 0usize;
    let mut pending = Vec::new();
    loop {
        if events::STOP.load(Ordering::Relaxed)
            || timeout.is_some_and(|s| start.elapsed() >= Duration::from_secs(s))
        {
            break;
        }
        if file.metadata()?.len() < offset {
            offset = 0;
            pending.clear();
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0u8; 65536];
        let n = file.read(&mut bytes)?;
        offset += n as u64;
        pending.extend_from_slice(&bytes[..n]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line = pending.drain(..=end).collect::<Vec<_>>();
            if !line.iter().all(u8::is_ascii_whitespace) {
                let value: Value = serde_json::from_slice(&line)
                    .context("Transcript is not JSONL (this provider may use a database)")?;
                crate::line(&value, out)?;
                emitted += 1;
                if count.is_some_and(|c| emitted >= c) {
                    return Ok(());
                }
            }
        }
        ensure!(
            pending.len() <= 16 * 1024 * 1024,
            "Transcript record exceeds 16 MiB"
        );
        if n == 0 {
            if !follow {
                if !pending.is_empty() {
                    let v: Value = serde_json::from_slice(&pending)
                        .context("Incomplete final transcript record")?;
                    line(&v, out)?;
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
fn terminal_follow(
    config: &Config,
    terminal: &str,
    cursor: Option<u64>,
    interval: u64,
    count: Option<usize>,
    timeout: Option<u64>,
    out: &mut impl Write,
) -> Result<()> {
    ensure!(interval >= 100, "Minimum interval is 100 ms");
    let rpc = RpcClient::new(config);
    let mut cursor = cursor;
    let start = Instant::now();
    let mut emitted = 0usize;
    while !events::STOP.load(Ordering::Relaxed)
        && timeout.is_none_or(|s| start.elapsed() < Duration::from_secs(s))
    {
        let mut params = json!({"terminal":terminal,"limit":65536});
        if let Some(cursor) = cursor {
            params["cursor"] = cursor.into();
        }
        let value = rpc.call("terminal.read", params)?;
        let next = value.get("nextCursor").and_then(Value::as_u64).or_else(|| {
            value
                .pointer("/terminal/nextCursor")
                .and_then(Value::as_u64)
        });
        if next != cursor || emitted == 0 {
            line(
                &json!({"topic":"terminal.output","terminal":terminal,"data":value}),
                out,
            )?;
            emitted += 1;
            if count.is_some_and(|c| emitted >= c) {
                break;
            }
        }
        cursor = next.or(cursor);
        std::thread::sleep(Duration::from_millis(interval));
    }
    Ok(())
}
fn run_rpc(config: &Config, r: Rpc, format: Format, out: &mut impl Write) -> Result<()> {
    let client = RpcClient::new(config);
    if r.pipe {
        let mut input_reader = io::stdin().lock();
        while let Some(input) = read_record(&mut input_reader)? {
            if input.trim().is_empty() {
                continue;
            }
            let request: Value =
                serde_json::from_str(&input).context("RPC pipe input must be JSONL")?;
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let result = request["method"]
                .as_str()
                .context("RPC input needs method")
                .and_then(|method| {
                    client.call(
                        method,
                        request.get("params").cloned().unwrap_or(Value::Null),
                    )
                });
            let response = match result {
                Ok(result) => json!({"id":id,"ok":true,"result":result}),
                Err(e) => json!({"id":id,"ok":false,"error":format!("{e:#}")}),
            };
            line(&response, out)?;
        }
        Ok(())
    } else {
        let method = r.method.context("Supply a method, or --pipe")?;
        let params: Value = serde_json::from_str(&if r.stdin { stdin_text()? } else { r.params })?;
        emit(&client.call(&method, params)?, format, out)
    }
}
fn command_effect(path: &[String]) -> &'static str {
    let p = path.iter().map(String::as_str).collect::<Vec<_>>();
    match p.as_slice() {
        ["automations", "create" | "edit" | "run" | "remove"] => "runtime-write",
        ["rpc", ..] | ["orca", ..] => "depends-on-method",
        ["stream", "pair"] => "local-credential-write",
        ["projects", "icon", "import"]
        | ["service", "install" | "uninstall"]
        | ["events", "prune"]
        | ["sessions", "rename"] => "local-write",
        ["skill", "install"] => "local-write",
        ["projects", "add"]
        | ["projects", "settings", "set"]
        | ["projects", "icon", "apply"]
        | ["sessions", "open" | "resume" | "create" | "send" | "stop"]
        | [
            "terminals",
            "focus" | "create" | "send" | "rename" | "stop" | "close",
        ]
        | ["worktrees", "create"]
        | ["open"] => "runtime-write",
        ["watch"] | ["events", "watch"] => "read-stream-with-optional-local-recording",
        ["stream", "subscribe"]
        | ["events", "follow"]
        | ["terminals", "follow"]
        | ["sessions", "transcript"] => "read-stream",
        _ => "read",
    }
}

fn describe_argument(command: &clap::Command, arg: &clap::Arg) -> Value {
    let allowed = arg.get_value_parser().possible_values().map(|values| {
        values
            .filter(|v| !v.is_hide_set())
            .map(|v| v.get_name().to_owned())
            .collect::<Vec<_>>()
    });
    json!({"name":arg.get_id().as_str(),"long":arg.get_long(),"short":arg.get_short(),
            "required":arg.is_required_set(),"global":arg.is_global_set(),
            "help":arg.get_long_help().or(arg.get_help()).map(ToString::to_string),
            "conflictsWith":command.get_arg_conflicts_with(arg).iter().map(|a|a.get_id().as_str()).collect::<Vec<_>>(),
            "action":format!("{:?}",arg.get_action()),
            "default":arg.get_default_values().iter().map(|v|v.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "allowedValues":allowed,
            "values":arg.get_num_args().map(|n|json!({"min":n.min_values(),"max":n.max_values()}))})
}

fn describe_command(command: &clap::Command, path: Vec<String>) -> Value {
    let args = command
        .get_arguments()
        .filter(|arg| !arg.is_global_set())
        .map(|arg| describe_argument(command, arg))
        .collect::<Vec<_>>();
    let children = command
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|child| {
            let mut p = path.clone();
            p.push(child.get_name().to_owned());
            describe_command(child, p)
        })
        .collect::<Vec<_>>();
    let mut usage_command = command.clone();
    json!({"name":command.get_name(),"path":path,"description":command.get_long_about().or(command.get_about()).map(ToString::to_string),
        "usage":usage_command.render_usage().to_string(),"effect":if command.has_subcommands() {"command-group"} else {command_effect(&path)},
        "aliases":command.get_visible_aliases().collect::<Vec<_>>(),"arguments":args,"commands":children,
        "examplesAndNotes":command.get_after_long_help().or(command.get_after_help()).map(ToString::to_string)})
}

fn command_help(path: &[String], as_json: bool) -> Result<()> {
    let mut root = Cli::command().color(terminal::color_choice());
    root.build();
    let mut command = &root;
    for part in path {
        command = command
            .find_subcommand(part)
            .with_context(|| format!("Unknown help command {part}"))?;
    }
    let mut out = io::stdout().lock();
    if as_json {
        line(
            &json!({"schemaVersion":1,"cliVersion":env!("CARGO_PKG_VERSION"),
            "contracts":{"stdout":"JSON when piped; streams always JSONL","stderr":"diagnostics","colorsInJson":false,
                "exitCodes":{"0":"success or downstream broken pipe","1":"runtime or input error","2":"CLI syntax error"},
                "unknownValues":"null means unknown, not zero or idle","runtimeHandles":"reacquire after runtime/incarnation change"},
            "globalOptions":root.get_arguments().filter(|arg|arg.is_global_set()).map(|arg|describe_argument(&root,arg)).collect::<Vec<_>>(),
            "command":describe_command(command,path.to_vec())}),
            &mut out,
        )
    } else {
        let mut command = command.clone();
        command.print_long_help()?;
        writeln!(out)?;
        Ok(())
    }
}

fn run() -> Result<()> {
    let choice = terminal::color_choice();
    let matches = Cli::command().color(choice).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;
    terminal::configure(choice);
    if let Command::Skill { command } = &cli.command {
        let mut out = io::stdout().lock();
        return match command {
            None if cli.json => line(
                &json!({"name":"orca-bridge","cliVersion":env!("CARGO_PKG_VERSION"),"markdown":skills::MARKDOWN}),
                &mut out,
            ),
            None => out
                .write_all(skills::MARKDOWN.as_bytes())
                .map_err(Into::into),
            Some(SkillCommand::Install { claude, .. }) => {
                let home = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?);
                let base = if *claude {
                    home.join(".claude")
                } else {
                    std::env::var_os("CODEX_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join(".codex"))
                };
                let path = skills::install(&base)?;
                line(
                    &json!({"installed":true,"path":path,"newSessionRequired":true}),
                    &mut out,
                )
            }
        };
    }
    if let Command::Schema { entity } = &cli.command {
        let schema = match entity {
            SchemaEntity::Session => schemars::schema_for!(Session),
            SchemaEntity::SessionView => schemars::schema_for!(ui::SessionView),
            SchemaEntity::Snapshot => schemars::schema_for!(ui::Snapshot),
        };
        return emit(
            &serde_json::to_value(schema)?,
            Format::Json,
            &mut io::stdout().lock(),
        );
    }
    if let Command::Help { path } = &cli.command {
        return command_help(path, cli.json);
    }
    if matches!(cli.command, Command::Agent) {
        let guide = include_str!("../docs/agent-guide.md");
        let mut out = io::stdout().lock();
        return if cli.json {
            line(&json!({"schemaVersion":1,"guide":guide}), &mut out)
        } else {
            out.write_all(guide.as_bytes()).map_err(Into::into)
        };
    }
    let mut config = Config::discover(cli.data_dir, cli.state_dir)?;
    config.profile_db_override = cli.profile_db;
    let format = if cli.alfred {
        Format::Alfred
    } else if cli.jsonl {
        Format::Jsonl
    } else if cli.human {
        Format::Text
    } else if cli.json || !io::stdout().is_terminal() {
        Format::Json
    } else {
        Format::Text
    };
    ctrlc::set_handler(|| events::STOP.store(true, Ordering::Relaxed))?;
    let mut out = io::stdout().lock();
    match cli.command {
        Command::Help { .. } | Command::Agent | Command::Schema { .. } | Command::Skill { .. } => {
            unreachable!()
        }
        Command::Snapshot { limit, icons } => emit(
            &serde_json::to_value(ui::snapshot(&config, limit, icons)?)?,
            format,
            &mut out,
        ),
        Command::Automations { command } => emit(&automation(&config, command)?, format, &mut out),
        Command::Sessions { command } => run_session(&config, command, format, &mut out),
        Command::Recent(list) => {
            run_session(&config, SessionCommand::Recent(list), format, &mut out)
        }
        Command::Search(s) => emit(&search(&config, s)?, format, &mut out),
        Command::Open { target } => {
            let id = if target.starts_with("orca-bridge:") {
                actions::parse_link(&target)?
            } else {
                target
            };
            emit(
                &actions::open_session(&config, &history::get_session(&config, &id)?)?,
                format,
                &mut out,
            )
        }
        Command::Projects { command } => {
            let value = match command {
                ProjectCommand::List => {
                    RpcClient::new(&config).call("project.list", Value::Null)?
                }
                ProjectCommand::Add { path } => {
                    RpcClient::new(&config).call("repo.add", json!({"path":path}))?
                }
                ProjectCommand::Settings { command } => match command {
                    ProjectSettingsCommand::Show { repo } => projects::show(&config, &repo)?,
                    ProjectSettingsCommand::Set {
                        repo,
                        updates,
                        stdin,
                    } => {
                        let updates = if stdin { stdin_text()? } else { updates };
                        projects::set(&config, &repo, serde_json::from_str(&updates)?)?
                    }
                },
                ProjectCommand::Icon { command } => match command {
                    ProjectIconCommand::Import { repo, file } => {
                        projects::import_icon(&config, &repo, &file)?
                    }
                    ProjectIconCommand::Apply { repo, file } => {
                        projects::apply_icon(&config, &repo, &file)?
                    }
                },
            };
            emit(&value, format, &mut out)
        }
        Command::Worktrees { command } => {
            let mut args = vec!["worktree".into()];
            match command {
                WorktreeCommand::List { project, repo } => {
                    args.push("list".into());
                    optional_arg(&mut args, "--repo", repo);
                    if let Some(project) = project {
                        let r = RpcClient::new(&config).call("worktree.list", json!({}))?;
                        let list = r["worktrees"]
                            .as_array()
                            .context("Missing worktrees")?
                            .iter()
                            .filter(|w| w["projectId"] == project)
                            .cloned()
                            .collect::<Vec<_>>();
                        return emit(&json!(list), format, &mut out);
                    }
                }
                WorktreeCommand::Show { selector } => {
                    args.extend(["show".into(), "--worktree".into(), selector])
                }
                WorktreeCommand::Create {
                    repo,
                    project,
                    name,
                    agent,
                    prompt,
                    activate,
                } => {
                    args.extend(["create".into(), "--name".into(), name]);
                    optional_arg(&mut args, "--repo", repo);
                    optional_arg(&mut args, "--project", project);
                    optional_arg(&mut args, "--agent", agent);
                    optional_arg(&mut args, "--prompt", prompt.map(literal).transpose()?);
                    if activate {
                        args.push("--activate".into());
                    }
                }
            }
            emit(&actions::orca(&config, &args)?, format, &mut out)
        }
        Command::Terminals { command } => {
            let mut args = vec!["terminal".into()];
            match command {
                TerminalCommand::Rename { terminal, title } => {
                    return emit(
                        &actions::rename_terminal(&config, &terminal, &title)?,
                        format,
                        &mut out,
                    );
                }
                TerminalCommand::Stop { terminal } => {
                    return emit(
                        &actions::stop_terminal(&config, &terminal, None)?,
                        format,
                        &mut out,
                    );
                }
                TerminalCommand::Close { terminal, tab } => {
                    args.extend(["close".into(), "--terminal".into(), terminal]);
                    if tab {
                        args.push("--tab".into());
                    }
                }
                TerminalCommand::List { worktree, layouts } => {
                    args.push("list".into());
                    optional_arg(&mut args, "--worktree", worktree);
                    if layouts {
                        args.push("--include-visual-layouts".into());
                    }
                }
                TerminalCommand::Show { terminal } => {
                    args.extend(["show".into(), "--terminal".into(), terminal])
                }
                TerminalCommand::Read {
                    terminal,
                    cursor,
                    limit,
                } => {
                    let r = RpcClient::new(&config).call(
                        "terminal.read",
                        match cursor {
                            Some(cursor) => {
                                json!({"terminal":terminal,"cursor":cursor,"limit":limit})
                            }
                            None => json!({"terminal":terminal,"limit":limit}),
                        },
                    )?;
                    return emit(&r, format, &mut out);
                }
                TerminalCommand::Follow {
                    terminal,
                    cursor,
                    interval_ms,
                    count,
                    timeout,
                } => {
                    return terminal_follow(
                        &config,
                        &terminal,
                        cursor,
                        interval_ms,
                        count,
                        timeout,
                        &mut out,
                    );
                }
                TerminalCommand::Focus { terminal } => {
                    args.extend(["switch".into(), "--terminal".into(), terminal])
                }
                TerminalCommand::Send {
                    terminal,
                    text,
                    enter,
                    interrupt,
                } => {
                    return emit(
                        &send(&config, terminal, text, enter, interrupt)?,
                        format,
                        &mut out,
                    );
                }
                TerminalCommand::Create {
                    worktree,
                    title,
                    command,
                    focus,
                } => {
                    args.push("create".into());
                    optional_arg(&mut args, "--worktree", worktree);
                    optional_arg(&mut args, "--title", title);
                    optional_arg(&mut args, "--command", command);
                    if focus {
                        args.push("--focus".into());
                    }
                }
            }
            emit(&actions::orca(&config, &args)?, format, &mut out)
        }
        Command::Watch(w) => run_watch(&config, w, &mut out),
        Command::Events { command } => match command {
            EventCommand::Watch(w) => run_watch(&config, w, &mut out),
            EventCommand::Status => emit(&events::status(&config)?, format, &mut out),
            EventCommand::Prune {
                keep_days,
                keep_count,
            } => emit(
                &events::prune(&config, keep_days, keep_count)?,
                format,
                &mut out,
            ),
            EventCommand::List {
                since_id,
                limit,
                topic,
            } => emit(
                &json!(events::replay(&config, since_id, limit, topic.as_deref())?),
                format,
                &mut out,
            ),
            EventCommand::Follow {
                mut since_id,
                topic,
                count,
                timeout,
            } => {
                let start = Instant::now();
                let mut emitted = 0usize;
                while !events::STOP.load(Ordering::Relaxed)
                    && timeout.is_none_or(|s| start.elapsed() < Duration::from_secs(s))
                {
                    for e in events::replay(&config, since_id, 100, topic.as_deref())? {
                        since_id = e["id"].as_i64().context("Journal event missing ID")?;
                        line(&e, &mut out)?;
                        emitted += 1;
                        if count.is_some_and(|c| emitted >= c) {
                            return Ok(());
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(())
            }
        },
        Command::Stream { command } => match command {
            StreamCommand::Status => emit(&stream::status(&config), format, &mut out),
            StreamCommand::Pair { stdin: _ } => emit(
                &stream::import_pairing(&config, &stdin_text()?)?,
                format,
                &mut out,
            ),
            StreamCommand::Subscribe {
                method,
                params,
                count,
                timeout,
                no_reconnect,
            } => {
                stream::load(&config)?;
                ensure!(count != Some(0), "--count must be positive");
                let params: Value = serde_json::from_str(&params)?;
                let start = Instant::now();
                let mut emitted = 0usize;
                let mut delay = Duration::from_millis(250);
                let mut reconnecting = false;
                if let Some(seconds) = timeout {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(seconds));
                        events::STOP.store(true, Ordering::Relaxed);
                    });
                }
                loop {
                    let result = stream::watch(
                        &config,
                        &method,
                        params.clone(),
                        &events::STOP,
                        |v| {
                            if reconnecting {
                                line(
                                    &json!({"topic":"transport","kind":"reconnected","reconcile":true}),
                                    &mut out,
                                )?;
                                reconnecting = false;
                            }
                            delay = Duration::from_millis(250);
                            line(&v, &mut out)?;
                            emitted += 1;
                            if count.is_some_and(|c| emitted >= c) {
                                events::STOP.store(true, Ordering::Relaxed);
                            }
                            Ok(())
                        },
                    );
                    if events::STOP.load(Ordering::Relaxed) {
                        return result;
                    }
                    if no_reconnect || result.is_ok() {
                        return result;
                    }
                    if let Err(err) = &result {
                        if !stream::retryable(err) {
                            return result;
                        }
                        line(
                            &json!({"topic":"transport","kind":"disconnected","retryAfterMs":delay.as_millis(),"error":format!("{err:#}")}),
                            &mut out,
                        )?;
                    }
                    if timeout.is_some_and(|s| start.elapsed() >= Duration::from_secs(s)) {
                        return Ok(());
                    }
                    reconnecting = true;
                    let until = Instant::now() + delay;
                    while !events::STOP.load(Ordering::Relaxed) && Instant::now() < until {
                        std::thread::sleep(
                            Duration::from_millis(50)
                                .min(until.saturating_duration_since(Instant::now())),
                        );
                    }
                    delay = (delay * 2).min(Duration::from_secs(10));
                    if events::STOP.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                }
            }
        },
        Command::Rpc(r) => run_rpc(&config, r, format, &mut out),
        Command::Filter { query, field } => {
            use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
            use nucleo_matcher::{Config as MatchConfig, Matcher, Utf32Str};
            let mut matcher = Matcher::new(MatchConfig::DEFAULT);
            let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
            let mut buffer = Vec::new();
            let mut input_reader = io::stdin().lock();
            while let Some(input) = read_record(&mut input_reader)? {
                let v: Value = serde_json::from_str(&input)?;
                let text = field
                    .as_deref()
                    .and_then(|f| v.pointer(f))
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_else(|| v.to_string());
                if pattern
                    .score(Utf32Str::new(&text, &mut buffer), &mut matcher)
                    .is_some()
                {
                    line(&v, &mut out)?;
                }
            }
            Ok(())
        }
        Command::Doctor => {
            let history = history::load_sessions(&config);
            let runtime = RpcClient::new(&config)
                .with_timeout(Duration::from_secs(2))
                .call("status.get", Value::Null);
            let r = json!({"version":env!("CARGO_PKG_VERSION"),"dataDir":config.data_dir,"stateDir":config.state_dir,"profileDatabase":config.profile_db().ok(),"history":{"ok":history.is_ok(),"sessions":history.as_ref().ok().map(Vec::len),"error":history.err().map(|e|format!("{e:#}"))},"runtime":{"ok":runtime.is_ok(),"status":runtime.as_ref().ok().map(|s|json!({"appVersion":s["appVersion"],"runtimeId":s["runtimeId"],"graphStatus":s["graphStatus"]})),"error":runtime.err().map(|e|format!("{e:#}"))},"nativeStream":stream::status(&config),"orcaCli":config.orca_cli,"compatibility":"Orca 1.4.225; read-only SQLite plus authenticated local RPC"});
            emit(&r, format, &mut out)
        }
        Command::Orca { args } => emit(&actions::orca(&config, &args)?, format, &mut out),
        Command::Service { command } => emit(&service(&config, command)?, format, &mut out),
    }
}
fn service(config: &Config, command: ServiceCommand) -> Result<Value> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?);
    let path = home.join("Library/LaunchAgents/dev.joel.orca-bridge.plist");
    let uid = std::process::Command::new("id").arg("-u").output()?;
    let uid = String::from_utf8(uid.stdout)?.trim().to_owned();
    let domain = format!("gui/{uid}");
    let label = "dev.joel.orca-bridge";
    match command {
        ServiceCommand::Status => {
            let r = std::process::Command::new("launchctl")
                .args(["print", &format!("{domain}/{label}")])
                .output()?;
            Ok(json!({"installed":path.is_file(),"running":r.status.success(),"plist":path}))
        }
        ServiceCommand::Uninstall => {
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("{domain}/{label}")])
                .output();
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            Ok(json!({"installed":false,"historyPreserved":true}))
        }
        ServiceCommand::Install => {
            let exe = std::env::current_exe()?;
            std::fs::create_dir_all(path.parent().context("Missing LaunchAgents directory")?)?;
            ensure!(
                !path.exists(),
                "Recorder already installed; uninstall it before changing configuration"
            );
            let args = vec![
                exe.to_string_lossy().into_owned(),
                "--data-dir".into(),
                config.data_dir.to_string_lossy().into_owned(),
                "--state-dir".into(),
                config.state_dir.to_string_lossy().into_owned(),
                "events".into(),
                "watch".into(),
                "--record".into(),
                "--no-initial".into(),
            ];
            fn xml(s: &str) -> String {
                s.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;")
            }
            let strings = args
                .iter()
                .map(|s| format!("<string>{}</string>", xml(s)))
                .collect::<String>();
            let plist = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{strings}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>StandardOutPath</key><string>/dev/null</string><key>StandardErrorPath</key><string>/dev/null</string></dict></plist>"
            );
            std::fs::write(&path, plist)?;
            let status = std::process::Command::new("launchctl")
                .arg("bootstrap")
                .arg(&domain)
                .arg(&path)
                .status()?;
            if !status.success() {
                std::fs::remove_file(&path)?;
                bail!("launchctl bootstrap failed");
            }
            Ok(json!({"installed":true,"plist":path,"executable":exe}))
        }
    }
}
fn main() {
    if let Err(e) = run() {
        if e.chain().any(|e| {
            e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
        }) {
            return;
        }
        eprintln!("orca-bridge: {e:#}");
        std::process::exit(1);
    }
}
