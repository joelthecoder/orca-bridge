//! Black-box contracts for machine-readable output and ordinary Unix pipes.
//! All Orca state here is generated; these tests never contact the installed app.
use rusqlite::params;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::UNIX_EPOCH;

struct Fixture {
    _directory: tempfile::TempDir,
    data: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("orca");
        let state = directory.path().join("bridge");
        std::fs::create_dir_all(data.join("ai-vault")).unwrap();
        let transcript = directory.path().join("fixture.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        let db = rusqlite::Connection::open(data.join("ai-vault/session-search.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE sessions(agent TEXT,session_id TEXT,title TEXT,cwd TEXT,branch TEXT,created_at TEXT,updated_at TEXT,message_count INTEGER,file_path TEXT,codex_home TEXT)").unwrap();
        for (agent, id, title, branch, updated) in [
            (
                "codex",
                "alpha",
                "Session bridge design",
                "feature/bridge",
                "2026-10-01T00:00:00Z",
            ),
            (
                "claude",
                "beta",
                "Réparer café",
                "feature/東京",
                "2026-10-21T00:00:00Z",
            ),
            (
                "codex",
                "gamma",
                "Session bridge implementation",
                "feature/bridge",
                "2026-10-20T00:00:00Z",
            ),
        ] {
            db.execute("INSERT INTO sessions VALUES(?1,?2,?3,'/fixture/project',?4,'2026-10-01T00:00:00Z',?5,3,?6,NULL)",params![agent,id,title,branch,updated,transcript.to_str().unwrap()]).unwrap();
        }
        drop(db);
        let metadata = transcript.metadata().unwrap();
        let cache = json!({"schemaVersion":4,"entries":[["fixture",{
            "mtimeMs":metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()*1000.0,
            "sizeBytes":metadata.len(),
            "session":{"agent":"codex","sessionId":"gamma","executionHostId":"local","filePath":transcript,"model":"gpt-fixture","totalTokens":1234}
        }]]});
        std::fs::write(
            data.join("ai-vault/session-parse-cache.json"),
            cache.to_string(),
        )
        .unwrap();
        Self {
            _directory: directory,
            data,
            state,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orca-bridge"));
        command
            .arg("--data-dir")
            .arg(&self.data)
            .arg("--state-dir")
            .arg(&self.state)
            .args(args)
            .env("ORCA_BRIDGE_ORCA_CLI", self.data.join("absent-orca"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str], input: &str) -> Output {
        let mut child = self.command(args).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "successful machine output has diagnostics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json_output(output: &Output) -> Value {
    succeeded(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn json_lines(output: &Output) -> Vec<Value> {
    succeeded(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn recent_defaults_to_json_in_a_pipe_and_jsonl_contains_individual_sessions() {
    let fixture = Fixture::new();
    let result = json_output(&fixture.run(&["sessions", "recent", "--limit", "2"], ""));
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(result[0]["id"], "local:claude:beta");
    assert_eq!(result[1]["id"], "local:codex:gamma");
    assert_eq!(result[1]["model"], "gpt-fixture");
    assert_eq!(result[1]["totalTokens"], 1234);
    let lines = json_lines(&fixture.run(&["sessions", "recent", "--limit", "2", "--jsonl"], ""));
    assert_eq!(lines, result.as_array().unwrap().to_vec());
}

#[test]
fn search_filters_agent_path_and_since_and_matches_unicode_and_model() {
    let fixture = Fixture::new();
    let result = json_output(&fixture.run(
        &[
            "sessions",
            "search",
            "bridge",
            "--agent",
            "codex",
            "--path",
            "/fixture",
            "--since",
            "2026-10-15T00:00:00Z",
            "--json",
        ],
        "",
    ));
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["sessionId"], "gamma");
    assert!(result[0]["score"].as_u64().unwrap() > 0);
    let result = json_lines(&fixture.run(&["sessions", "search", "cafe 東京", "--jsonl"], ""));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["sessionId"], "beta");
    let result = json_output(&fixture.run(&["search", "gpt-fixture", "--json"], ""));
    assert_eq!(result[0]["sessionId"], "gamma");
}

#[test]
fn project_path_filter_matches_directories_and_descendants_without_prefix_collisions() {
    let fixture = Fixture::new();
    let db =
        rusqlite::Connection::open(fixture.data.join("ai-vault/session-search.sqlite")).unwrap();
    for (id, cwd) in [
        ("alpha", "/fixture/project"),
        ("beta", "/fixture/project/subdir"),
        ("gamma", "/fixture/project-neighbor"),
    ] {
        db.execute(
            "UPDATE sessions SET cwd=?1 WHERE session_id=?2",
            params![cwd, id],
        )
        .unwrap();
    }
    for command in ["recent", "search"] {
        let result =
            json_output(&fixture.run(&[command, "--path", "/fixture/project", "--json"], ""));
        let rows = result.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row["sessionId"] != "gamma"));
    }
    let source = fixture.run(&["recent", "--jsonl"], "");
    let result = json_lines(&fixture.run(
        &[
            "search",
            "--stdin",
            "--path",
            "/fixture/project/",
            "--jsonl",
        ],
        std::str::from_utf8(&source.stdout).unwrap(),
    ));
    assert_eq!(result.len(), 2);
}

fn fixture_transcript(fixture: &Fixture) -> PathBuf {
    fixture._directory.path().join("fixture.jsonl")
}

#[test]
fn transcript_last_reads_recent_records_in_order_without_parsing_old_history() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture_transcript(&fixture),
        "old invalid record\n{\"n\":1}\n\n{\"n\":2}\r\n \t\n{\"n\":3}",
    )
    .unwrap();
    let args = ["sessions", "transcript", "alpha", "--last", "2"];
    assert_eq!(
        json_lines(&fixture.run(&args, "")),
        vec![json!({"n":2}), json!({"n":3})]
    );
    assert_eq!(
        json_lines(&fixture.run(&[&args[..], &["--count", "1"]].concat(), "")),
        vec![json!({"n":2})]
    );
    std::fs::write(fixture_transcript(&fixture), "{\"n\":1}\n \t").unwrap();
    assert_eq!(
        json_lines(&fixture.run(&["sessions", "transcript", "alpha", "--last", "20"], "")),
        vec![json!({"n":1})]
    );
    std::fs::write(fixture_transcript(&fixture), "").unwrap();
    assert!(json_lines(&fixture.run(&args, "")).is_empty());
}

#[test]
fn transcript_rejects_zero_counts_conflicting_modes_and_oversized_complete_records() {
    let fixture = Fixture::new();
    for flags in [
        vec!["--last", "0"],
        vec!["--count", "0"],
        vec!["--last", "2", "--tail"],
    ] {
        let output = fixture.run(
            &[&["sessions", "transcript", "alpha"][..], &flags].concat(),
            "",
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    std::fs::write(
        fixture_transcript(&fixture),
        format!("\"{}\"\n", "x".repeat(16 * 1024 * 1024)),
    )
    .unwrap();
    let output = fixture.run(&["sessions", "transcript", "alpha", "--last", "1"], "");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("exceeds 16 MiB")
    );
}

#[test]
fn transcript_last_follow_keeps_partial_record_until_it_is_completed() {
    use std::io::{BufRead, BufReader};
    let fixture = Fixture::new();
    let path = fixture_transcript(&fixture);
    std::fs::write(&path, "{\"n\":0}\n{\"n\":1}\n{\"n\":").unwrap();
    let mut child = fixture
        .command(&[
            "sessions",
            "transcript",
            "alpha",
            "--last",
            "1",
            "--follow",
            "--count",
            "2",
            "--timeout",
            "3",
        ])
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"n":1})
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"2}\n").unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"n":2})
    );
    succeeded(&child.wait_with_output().unwrap());
}

#[test]
fn recent_messages_project_text_with_role_limit_and_explicit_truncation() {
    let fixture = Fixture::new();
    let records = [
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Working"}]}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":"duplicate"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final","content":[{"type":"output_text","text":"Answer"}]}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"tool output"}}),
    ];
    let text = records
        .iter()
        .map(|r| r.to_string() + "\n")
        .collect::<String>();
    std::fs::write(fixture_transcript(&fixture), text).unwrap();
    let result = json_output(&fixture.run(
        &[
            "sessions",
            "messages",
            "alpha",
            "--role",
            "assistant",
            "--limit",
            "1",
        ],
        "",
    ));
    assert_eq!(result["id"], "local:codex:alpha");
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["text"], "Answer");
    assert_eq!(result["messages"][0]["phase"], "final");
    assert!(result["messages"][0]["sourceOffset"].as_u64().unwrap() > 0);
    assert_eq!(result["limitTruncated"], true);
    assert_eq!(result["sourceTruncated"], false);
    let result =
        json_output(&fixture.run(&["sessions", "messages", "alpha", "--max-bytes", "1"], ""));
    assert_eq!(result["sourceTruncated"], true);
    assert_eq!(result["messages"], json!([]));
    for args in [
        vec!["--limit", "0"],
        vec!["--role", "tool"],
        vec!["--max-bytes", "0"],
        vec!["--max-bytes", "16777217"],
    ] {
        let output = fixture.run(
            &[&["sessions", "messages", "alpha"][..], &args].concat(),
            "",
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn stdin_search_accepts_jsonl_and_arrays_without_accessing_the_index() {
    let fixture = Fixture::new();
    let source = fixture.run(&["sessions", "recent", "--jsonl"], "");
    succeeded(&source);
    std::fs::remove_file(fixture.data.join("ai-vault/session-search.sqlite")).unwrap();
    let result = json_lines(&fixture.run(
        &["sessions", "search", "bridge", "--stdin", "--jsonl"],
        std::str::from_utf8(&source.stdout).unwrap(),
    ));
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|s| s["agent"] == "codex"));
    let array = serde_json::to_string(&result).unwrap();
    let result =
        json_output(&fixture.run(&["search", "implementation", "--stdin", "--json"], &array));
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["sessionId"], "gamma");
}

#[test]
fn alfred_items_have_action_arguments_and_stable_uids() {
    let fixture = Fixture::new();
    let result = json_output(&fixture.run(&["sessions", "search", "gpt-fixture", "--alfred"], ""));
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "local:codex:gamma");
    assert_eq!(items[0]["arg"], items[0]["uid"]);
    assert_eq!(items[0]["title"], "Session bridge implementation");
    assert_eq!(items[0]["valid"], true);
    assert!(
        items[0]["subtitle"]
            .as_str()
            .unwrap()
            .contains("gpt-fixture")
    );
}

#[test]
fn arbitrary_jsonl_filter_preserves_records_and_uses_the_selected_field() {
    let fixture = Fixture::new();
    let input = concat!(
        "{\"id\":1,\"title\":\"Bridge sessions\",\"nested\":{\"keep\":true}}\n",
        "{\"id\":2,\"title\":\"Unrelated\",\"other\":\"Bridge sessions\"}\n"
    );
    let rows = json_lines(&fixture.run(&["filter", "brg sess", "--field", "/title"], input));
    assert_eq!(
        rows,
        vec![json!({"id":1,"title":"Bridge sessions","nested":{"keep":true}})]
    );
}

#[test]
fn rpc_pipe_correlates_each_failure_and_continues_processing() {
    let fixture = Fixture::new();
    let rows = json_lines(&fixture.run(
        &["rpc", "--pipe"],
        concat!(
            "{\"id\":\"first\",\"method\":\"status.get\",\"params\":{}}\n",
            "{\"id\":42,\"method\":\"terminal.list\"}\n",
            "{\"id\":\"missing-method\"}\n"
        ),
    ));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["id"], "first");
    assert_eq!(rows[1]["id"], 42);
    assert_eq!(rows[2]["id"], "missing-method");
    assert!(rows.iter().all(|row| row["ok"] == false));
    assert!(
        rows[0]["error"]
            .as_str()
            .unwrap()
            .contains("runtime metadata unavailable")
    );
    assert!(rows[2]["error"].as_str().unwrap().contains("needs method"));
}

#[test]
fn malformed_stdin_fails_with_diagnostics_only_on_stderr() {
    let fixture = Fixture::new();
    for (args, input, context) in [
        (
            vec!["rpc", "--pipe"],
            "not JSON\n",
            Some("RPC pipe input must be JSONL"),
        ),
        (vec!["search", "bridge", "--stdin"], "{broken}\n", None),
        (vec!["filter", "bridge"], "{broken}\n", None),
        (
            vec!["recent", "--since", "yesterday"],
            "",
            Some("--since must be ISO 8601"),
        ),
    ] {
        let output = fixture.run(&args, input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostics = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostics.starts_with("orca-bridge: "));
        if let Some(context) = context {
            assert!(diagnostics.contains(context), "{diagnostics}");
        }
    }
}

#[test]
fn closed_downstream_pipe_is_successful_and_silent() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command(&["filter", "bridge", "--field", "/title"])
        .spawn()
        .unwrap();
    // Close the only reader before delivering any input, so the child's first
    // matching output must hit EPIPE, independent of pipe-buffer scheduling.
    drop(child.stdout.take().unwrap());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"title\":\"bridge\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn help_is_machine_discoverable_without_home_or_orca() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&["help", "--json"])
        .env_remove("HOME")
        .output()
        .unwrap();
    let help = json_output(&output);
    assert_eq!(help["schemaVersion"], 1);
    let commands = help["command"]["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c["name"] == "projects"));
    assert_eq!(
        commands.iter().find(|c| c["name"] == "open").unwrap()["effect"],
        "runtime-write"
    );
    let send = json_output(&fixture.run(&["help", "terminals", "send", "--json"], ""));
    assert_eq!(send["command"]["path"], json!(["terminals", "send"]));
    assert!(
        send["command"]["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["long"] == "text")
    );
    let guide = fixture
        .command(&["agent"])
        .env_remove("HOME")
        .output()
        .unwrap();
    succeeded(&guide);
    assert!(String::from_utf8_lossy(&guide.stdout).contains("Never add approval"));
}

#[test]
fn terminal_colors_are_explicit_and_never_enter_json() {
    let fixture = Fixture::new();
    let styled = fixture.run(&["recent", "--human", "--color", "always"], "");
    succeeded(&styled);
    assert!(styled.stdout.contains(&0x1b));
    let plain = fixture.run(&["recent", "--human", "--color", "never"], "");
    succeeded(&plain);
    assert!(!plain.stdout.contains(&0x1b));
    let json = fixture.run(&["recent", "--json", "--color", "always"], "");
    json_output(&json);
    assert!(!json.stdout.contains(&0x1b));
    let help = fixture.run(&["--color", "always", "--help"], "");
    succeeded(&help);
    assert!(help.stdout.contains(&0x1b));
    let no_color = fixture
        .command(&["--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    succeeded(&no_color);
    assert!(!no_color.stdout.contains(&0x1b));
}

impl Fixture {
    fn rpc_server(
        &self,
        count: usize,
        mut reply: impl FnMut(usize, Value) -> Value + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;
        let path = self.data.join("rpc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::write(self.data.join("orca-runtime.json"),json!({"runtimeId":"fixture","authToken":"fixture-only","transports":[{"kind":"unix","endpoint":path}]}).to_string()).unwrap();
        std::thread::spawn(move || {
            for index in 0..count {
                let (mut socket, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = reply(index, request.clone());
                writeln!(socket,"{}",json!({"id":request["id"],"ok":true,"result":result,"_meta":{"runtimeId":"fixture"}})).unwrap();
            }
        })
    }
}

#[test]
fn session_terminal_link_uses_only_the_proven_live_session() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(4, |_, request| match request["method"].as_str() {
        Some("terminal.list") => {
            assert_eq!(request["params"]["requireFreshPtyLiveness"], true);
            json!({"terminals":[
                {"handle":"term_alpha","incarnationId":"inc-alpha","agentIdentity":"codex","connected":true},
                {"handle":"term_gamma","incarnationId":"inc-gamma","agentIdentity":"codex","connected":true}
            ]})
        }
        Some("session.tabs.listAll") => json!({"snapshots":[{"tabs":[
            {"type":"terminal","terminal":"term_alpha","incarnationId":"inc-alpha","agentStatus":{"agentType":"codex","providerSession":{"id":"alpha"}}},
            {"type":"terminal","terminal":"term_gamma","incarnationId":"inc-gamma","agentStatus":{"agentType":"codex","providerSession":{"id":"gamma"}}}
        ]}]}),
        other => panic!("Link resolution must not mutate the runtime: {other:?}"),
    });
    let result =
        json_output(&fixture.run(&["sessions", "link", "alpha", "--terminal", "--json"], ""));
    assert_eq!(result["id"], "local:codex:alpha");
    assert_eq!(result["terminal"], "term_alpha");
    assert_eq!(result["identity"]["incarnationId"], "inc-alpha");
    assert_eq!(result["url"], "orca-bridge://session/local:codex:alpha");
    let output = fixture.run(&["sessions", "link", "alpha", "--terminal", "--human"], "");
    succeeded(&output);
    assert_eq!(output.stdout, b"term_alpha\n");
    server.join().unwrap();
}

#[test]
fn session_terminal_link_refuses_missing_or_ambiguous_ownership() {
    for count in [0, 2] {
        let fixture = Fixture::new();
        let server = fixture.rpc_server(2, move |_, request| {
            match request["method"].as_str() {
                Some("terminal.list") => json!({"terminals":(0..count).map(|i| json!({
                    "handle":format!("term_{i}"),"incarnationId":format!("inc-{i}"),
                    "agentIdentity":"codex","connected":true
                })).collect::<Vec<_>>()}),
                Some("session.tabs.listAll") => json!({"snapshots":[{"tabs":(0..count).map(|i| json!({
                    "type":"terminal","terminal":format!("term_{i}"),"incarnationId":format!("inc-{i}"),
                    "agentStatus":{"agentType":"codex","providerSession":{"id":"alpha"}}
                })).collect::<Vec<_>>()}]}),
                other => panic!("Link resolution must not mutate the runtime: {other:?}"),
            }
        });
        let output = fixture.run(&["sessions", "link", "alpha", "--terminal", "--json"], "");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("exactly one proven live terminal")
        );
        server.join().unwrap();
    }
}

#[test]
fn durable_session_links_still_work_without_a_running_runtime() {
    let fixture = Fixture::new();
    let result = json_output(&fixture.run(&["sessions", "link", "alpha", "--json"], ""));
    assert_eq!(
        result,
        json!({"id":"local:codex:alpha","url":"orca-bridge://session/local:codex:alpha"})
    );
}

#[test]
fn tab_move_refuses_an_old_host_before_showing_or_mutating_a_terminal() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(1, |_, request| {
        assert_eq!(request["method"], "status.get");
        json!({"capabilities": []})
    });
    let output = fixture.run(
        &[
            "terminals",
            "move",
            "term-selected",
            "--worktree",
            "path:/destination",
        ],
        "",
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("terminal.tab-transfer.v1"));
    assert!(!fixture.state.exists());
    server.join().unwrap();
}

#[test]
fn tab_move_forwards_identity_fences_and_literal_destination_to_native_owner() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(3, |index, request| match index {
        0 => {
            assert_eq!(request["method"], "status.get");
            json!({"capabilities": ["terminal.tab-transfer.v1"]})
        }
        1 => {
            assert_eq!(request["method"], "terminal.show");
            assert_eq!(request["params"], json!({"terminal": "term-selected"}));
            json!({"terminal": {"incarnationId": "inc-selected", "worktreeId": "source"}})
        }
        _ => {
            assert_eq!(request["method"], "terminal.transferTab");
            assert_eq!(request["params"], json!({
                "terminal": "term-selected", "destination": "path:/folder with spaces/$(literal)",
                "expectedIncarnationId": "inc-selected", "expectedWorktreeId": "source", "requestId": "move-fixture"
            }));
            json!({"moved": true, "requestId": "move-fixture", "ptyId": "unchanged"})
        }
    });
    let result = json_output(&fixture.run(
        &[
            "terminals",
            "move",
            "term-selected",
            "--worktree",
            "path:/folder with spaces/$(literal)",
            "--request-id",
            "move-fixture",
        ],
        "",
    ));
    assert_eq!(result["ptyId"], "unchanged");
    assert!(!fixture.state.exists());
    server.join().unwrap();
}

#[test]
fn selected_naming_preview_uses_provider_identity_without_writing_labels() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(2, |index, request| {
        if index == 0 {
            assert_eq!(request["method"], "status.get");
            return json!({"capabilities": ["session.names.v1"]});
        }
        assert_eq!(request["method"], "session.names.preview");
        assert_eq!(
            request["params"],
            json!({"selection": [{
                "session": {"agent": "codex", "sessionId": "alpha", "executionHostId": "local"},
                "title": "Stable task", "replaceManual": false
            }]})
        );
        json!({"planId": "native-plan", "items": [{"disposition": "preserve-manual"}]})
    });
    let result = json_output(&fixture.run(
        &["sessions", "names", "preview", "--stdin"],
        r#"[{"session":"local:codex:alpha","title":"Stable task"}]"#,
    ));
    assert_eq!(result["planId"], "native-plan");
    assert!(!fixture.state.exists());
    server.join().unwrap();
}

#[test]
fn naming_apply_and_undo_preserve_native_conflict_receipts() {
    for (command, field, id) in [
        ("apply", "planId", "native-plan"),
        ("undo", "changeId", "native-change"),
    ] {
        let fixture = Fixture::new();
        let method = format!("session.names.{command}");
        let server = fixture.rpc_server(2, move |index, request| {
            if index == 0 {
                return json!({"capabilities": ["session.names.v1"]});
            }
            assert_eq!(request["method"], method);
            assert_eq!(request["params"], json!({field: id}));
            json!({"applied": false, "conflicts": [{"reason": "title_changed"}]})
        });
        let result = json_output(&fixture.run(&["sessions", "names", command, id], ""));
        assert_eq!(result["applied"], false);
        assert_eq!(result["conflicts"][0]["reason"], "title_changed");
        assert!(!fixture.state.exists());
        server.join().unwrap();
    }
}

#[test]
fn naming_rejects_ambiguous_duplicate_and_unbounded_selections_before_rpc() {
    let fixture = Fixture::new();
    for selection in [
        json!([]),
        json!([{"session":"local:codex:alpha","title":"Stable","unexpected":true}]),
        json!([{"session":"local:codex:alpha","title":"Stable"},{"session":"alpha","title":"Other"}]),
        json!([{"session":"local:codex:alpha","title":"\u{001b}[2J"}]),
        json!(vec![
            json!({"session":"local:codex:alpha","title":"Stable"});
            101
        ]),
    ] {
        let output = fixture.run(
            &["sessions", "names", "preview", "--stdin"],
            &selection.to_string(),
        );
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime metadata"));
        assert!(!fixture.state.exists());
    }
}

#[test]
fn organization_commands_publish_runtime_effects_and_require_explicit_targets() {
    let fixture = Fixture::new();
    for path in [
        vec!["help", "terminals", "move", "--json"],
        vec!["help", "sessions", "names", "preview", "--json"],
        vec!["help", "sessions", "names", "apply", "--json"],
        vec!["help", "sessions", "names", "undo", "--json"],
    ] {
        let result = json_output(&fixture.run(&path, ""));
        assert_eq!(result["command"]["effect"], "runtime-write");
    }
    for args in [
        vec!["terminals", "move", "term-selected"],
        vec!["sessions", "names", "preview"],
        vec!["sessions", "names", "apply"],
        vec!["sessions", "names", "undo"],
    ] {
        let output = fixture.run(&args, "");
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn notification_epoch_is_omitted_until_known_then_forwarded() {
    let fixture = Fixture::new();
    let server=fixture.rpc_server(2,|index,request| {
        assert_eq!(request["method"],"notifications.getMissedSince");
        if index==0 { assert!(request["params"].get("epoch").is_none()); }
        else { assert_eq!(request["params"]["epoch"],"fixture-epoch"); assert_eq!(request["params"]["lastSeenSeq"],1); }
        json!({"epoch":"fixture-epoch","notifications":[{"notificationSeq":index+1,"title":"fixture"}]})
    });
    let rows = json_lines(&fixture.run(
        &[
            "watch",
            "--topics",
            "notifications",
            "--count",
            "2",
            "--interval-ms",
            "100",
            "--timeout",
            "3",
        ],
        "",
    ));
    assert_eq!(rows.len(), 2);
    server.join().unwrap();
}

#[test]
fn terminal_initial_cursor_is_omitted_rather_than_null() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(1, |_, request| {
        assert_eq!(request["method"], "terminal.read");
        assert!(request["params"].get("cursor").is_none());
        json!({"nextCursor":4,"data":"test"})
    });
    let rows = json_lines(&fixture.run(
        &[
            "terminals",
            "follow",
            "term_fixture",
            "--count",
            "1",
            "--timeout",
            "3",
        ],
        "",
    ));
    assert_eq!(rows[0]["data"]["nextCursor"], 4);
    server.join().unwrap();
}

#[test]
fn terminal_tail_read_preserves_native_cursors_draft_and_unknown_fields() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(1, |_, request| {
        assert_eq!(request["params"]["cursor"], "opaque:cursor");
        json!({"terminal":{"handle":"term_fixture","nextCursor":"opaque:next","draft":"keep this draft","tail":["old","recent","latest"],"future":42},"extra":true})
    });
    let result = json_output(&fixture.run(
        &[
            "terminals",
            "read",
            "term_fixture",
            "--cursor",
            "opaque:cursor",
            "--tail",
            "2",
        ],
        "",
    ));
    assert_eq!(result["terminal"]["tail"], json!(["recent", "latest"]));
    assert_eq!(result["terminal"]["draft"], "keep this draft");
    assert_eq!(result["terminal"]["nextCursor"], "opaque:next");
    assert_eq!(result["terminal"]["future"], 42);
    assert_eq!(result["extra"], true);
    assert_eq!(
        result["bridgeTail"],
        json!({"limit":2,"returned":2,"truncated":true})
    );
    server.join().unwrap();
}

#[test]
fn terminal_follow_resumes_opaque_cursor_and_waits_for_changed_output() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(2, |index, request| {
        assert_eq!(request["params"]["cursor"], "opaque:start");
        if index == 0 {
            json!({"terminal":{"nextCursor":"opaque:start","tail":[]}})
        } else {
            json!({"terminal":{"nextCursor":"opaque:next","tail":["new output"]}})
        }
    });
    let rows = json_lines(&fixture.run(
        &[
            "terminals",
            "follow",
            "term_fixture",
            "--cursor",
            "opaque:start",
            "--count",
            "1",
            "--interval-ms",
            "100",
            "--timeout",
            "3",
        ],
        "",
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["data"]["terminal"]["tail"], json!(["new output"]));
    server.join().unwrap();
}

#[test]
fn terminal_follow_carries_native_string_cursor_into_subsequent_reads() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(2, |index, request| {
        if index == 0 {
            assert!(request["params"].get("cursor").is_none());
        } else {
            assert_eq!(request["params"]["cursor"], "v1:first");
        }
        json!({"terminal":{"nextCursor":if index == 0 {"v1:first"} else {"v1:second"},"tail":["output"]}})
    });
    let rows = json_lines(&fixture.run(
        &[
            "terminals",
            "follow",
            "term_fixture",
            "--count",
            "2",
            "--interval-ms",
            "100",
            "--timeout",
            "3",
        ],
        "",
    ));
    assert_eq!(rows.len(), 2);
    server.join().unwrap();
}

#[test]
fn terminal_native_decimal_string_cursors_are_sent_as_rpc_numbers() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(3, |index, request| {
        assert_eq!(request["params"]["cursor"], if index < 2 { 42 } else { 43 });
        json!({"terminal":{"nextCursor":if index == 2 {"44"} else {"43"},"tail":["output"]}})
    });
    let result =
        json_output(&fixture.run(&["terminals", "read", "term_fixture", "--cursor", "42"], ""));
    assert_eq!(result["terminal"]["nextCursor"], "43");
    let rows = json_lines(&fixture.run(
        &[
            "terminals",
            "follow",
            "term_fixture",
            "--cursor",
            "42",
            "--count",
            "2",
            "--interval-ms",
            "100",
            "--timeout",
            "3",
        ],
        "",
    ));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["data"]["terminal"]["nextCursor"], "44");
    server.join().unwrap();
}

#[test]
fn terminal_follow_timeout_bounds_poll_sleep_and_zero_count_is_rejected() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(
        1,
        |_, _| json!({"terminal":{"nextCursor":"1","tail":["output"]}}),
    );
    let start = std::time::Instant::now();
    let rows = json_lines(&fixture.run(
        &[
            "terminals",
            "follow",
            "term_fixture",
            "--interval-ms",
            "3000",
            "--timeout",
            "1",
        ],
        "",
    ));
    assert_eq!(rows.len(), 1);
    assert!(start.elapsed() < std::time::Duration::from_millis(2500));
    server.join().unwrap();
    let output = fixture.run(&["terminals", "follow", "term_fixture", "--count", "0"], "");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--count must be positive")
    );
}

#[test]
fn terminal_follow_timeout_bounds_a_stalled_runtime_read() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    let fixture = Fixture::new();
    let path = fixture.data.join("rpc.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::write(fixture.data.join("orca-runtime.json"), json!({"runtimeId":"fixture","authToken":"fixture-only","transports":[{"kind":"unix","endpoint":path}]}).to_string()).unwrap();
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&socket).read_line(&mut line).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2500));
    });
    let start = std::time::Instant::now();
    let rows = json_lines(&fixture.run(
        &["terminals", "follow", "term_fixture", "--timeout", "1"],
        "",
    ));
    assert!(rows.is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    server.join().unwrap();
}

#[test]
fn project_patch_pins_repository_id_and_preserves_unrelated_fields() {
    let fixture = Fixture::new();
    let original = json!({"id":"fixture-repo","path":"/fixture/project","displayName":"Before","hookSettings":{"mode":"auto"},"futureField":{"preserved":true}});
    let mut changed = original.clone();
    changed["displayName"] = json!("After");
    let expected = changed.clone();
    let server = fixture.rpc_server(3, move |index, request| {
        if index == 0 {
            assert_eq!(request["params"]["repo"], "name:Before");
            return json!({"repo":original});
        }
        assert_eq!(request["params"]["repo"], "id:fixture-repo");
        if index == 1 {
            assert_eq!(request["method"], "repo.update");
            assert_eq!(request["params"]["updates"], json!({"displayName":"After"}));
        }
        json!({"repo":changed})
    });
    let actual = json_output(&fixture.run(
        &[
            "projects",
            "settings",
            "set",
            "name:Before",
            "--updates",
            "{\"displayName\":\"After\"}",
            "--json",
        ],
        "",
    ));
    assert_eq!(actual, expected);
    server.join().unwrap();
}

#[test]
fn unbounded_jsonl_record_is_rejected_before_parsing() {
    let fixture = Fixture::new();
    let input = "x".repeat(8 * 1024 * 1024 + 1);
    for args in [&["filter", "x"][..], &["rpc", "--pipe"][..]] {
        let output = fixture.run(args, &input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("record exceeds 8 MiB"));
    }
}

#[test]
fn automation_prompt_is_literal_and_schedule_validation_stays_with_orca() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let executable = fixture.data.join("fake-orca");
    std::fs::write(&executable,"#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$ORCA_USER_DATA_PATH/argv\"\nprintf '%s\\n' '{\"ok\":true,\"result\":{\"created\":true}}'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = fixture.command(&[
        "automations",
        "create",
        "--name",
        "Review",
        "--trigger",
        "daily",
        "--agent",
        "claude",
        "--prompt",
        "-",
        "--repo",
        "name:project",
        "--time",
        "09:00",
        "--timezone",
        "America/Edmonton",
        "--disabled",
        "--json",
    ]);
    command.env("ORCA_BRIDGE_ORCA_CLI", executable);
    let mut child = command.spawn().unwrap();
    let prompt = "literal $(not-a-command); `neither`\ncontext";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    assert_eq!(
        json_output(&child.wait_with_output().unwrap())["created"],
        true
    );
    let bytes = std::fs::read(fixture.data.join("argv")).unwrap();
    let args = bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![
            "automations",
            "create",
            "--name",
            "Review",
            "--trigger",
            "daily",
            "--provider",
            "claude",
            "--prompt",
            prompt,
            "--repo",
            "name:project",
            "--time",
            "09:00",
            "--timezone",
            "America/Edmonton",
            "--disabled",
            "--json"
        ]
    );
}

#[test]
fn ui_snapshot_preserves_history_and_invalidates_live_data_on_runtime_change() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(7, |index, request| {
        match request["method"].as_str().unwrap() {
            "status.get" => {
                json!({"runtimeId":if index==0 {"one"} else {"two"},"appVersion":"fixture"})
            }
            "project.list" => json!({"projects":[]}),
            "repo.list" => json!({"repos":[]}),
            "worktree.list" => json!({"worktrees":[],"truncated":false}),
            "terminal.list" => json!({"terminals":[]}),
            "session.tabs.listAll" => json!({"snapshots":[]}),
            method => panic!("Unexpected {method}"),
        }
    });
    let snapshot = json_output(&fixture.run(&["snapshot", "--limit", "2", "--json"], ""));
    assert_eq!(snapshot["schemaVersion"], 1);
    assert_eq!(snapshot["sessions"]["available"], true);
    assert_eq!(snapshot["historyTruncated"], true);
    assert_eq!(snapshot["sessions"]["data"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["terminals"]["available"], false);
    assert!(snapshot["terminals"]["data"].is_null());
    assert_eq!(snapshot["sessions"]["data"][0]["liveState"], "unknown");
    server.join().unwrap();
}

#[test]
fn schemas_and_bundled_skill_do_not_require_home_or_runtime() {
    let fixture = Fixture::new();
    let schema = json_output(
        &fixture
            .command(&["schema", "session"])
            .env_remove("HOME")
            .output()
            .unwrap(),
    );
    assert_eq!(schema["type"], "object");
    for key in ["id", "agent", "title", "cwd", "model", "totalTokens"] {
        assert!(schema["properties"].get(key).is_some());
    }
    let skill = fixture
        .command(&["skill"])
        .env_remove("HOME")
        .output()
        .unwrap();
    succeeded(&skill);
    assert!(String::from_utf8_lossy(&skill.stdout).starts_with("---\nname: orca-bridge\n"));
}

#[test]
fn terminal_stop_targets_exact_pty_and_preserves_history() {
    let fixture = Fixture::new();
    let server=fixture.rpc_server(2,|index,request| {
        if index==0 {assert_eq!(request["method"],"terminal.show");return json!({"terminal":{"handle":"term-fixture","ptyId":"pty-fixture","worktreeId":"tree-fixture","incarnationId":"inc-fixture"}});}
        assert_eq!(request["method"],"terminal.stopExact");
        assert_eq!(request["params"],json!({"worktree":"tree-fixture","expectedPtyIds":["pty-fixture"],"targetOnly":true,"keepHistory":true}));
        json!({"stopped":1,"stoppedPtyIds":["pty-fixture"],"postStopVerified":true})
    });
    assert_eq!(
        json_output(&fixture.run(&["terminals", "stop", "term-fixture", "--json"], ""))["stopped"],
        1
    );
    server.join().unwrap();
}

#[test]
fn resource_snapshot_exposes_sorted_usage_and_verified_stop_targets_without_raw_history() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(4, |index, request| match index {
        0 | 3 => { assert_eq!(request["method"], "status.get"); json!({"runtimeId":"fixture"}) },
        1 => { assert_eq!(request["method"], "diagnostics.memory"); assert!(request["params"].is_null());
            json!({"collectedAt":1,"processMemoryMetric":"rss","app":{"cpu":1.5,"memory":10},"host":{"totalMemory":1000,"availableMemory":500},"totalMemory":610,"totalCpu":5,"worktrees":[{"worktreeId":"tree-fixture","worktreeName":"Fixture","sessions":[{"sessionId":"pty-small","pid":10,"memory":100,"cpu":0},{"sessionId":"pty-big","pid":11,"memory":500,"cpu":3.5}],"history":["private fixture history"]}]}) },
        _ => { assert_eq!(request["method"], "terminal.list"); json!({"terminals":[{"handle":"term-small","ptyId":"pty-small","worktreeId":"tree-fixture","incarnationId":"inc-small","executionHostId":"local","connected":true},{"handle":"term-big","ptyId":"pty-big","worktreeId":"tree-fixture","incarnationId":"inc-big","executionHostId":"local","connected":true}],"truncated":false}) }
    });
    let result = json_output(&fixture.run(&["resources", "--limit", "1"], ""));
    assert_eq!(result["terminals"][0]["terminal"], "term-big");
    assert_eq!(result["terminals"][0]["incarnationId"], "inc-big");
    assert_eq!(result["terminals"][0]["memoryBytes"], 500);
    assert_eq!(result["terminalCount"], 2);
    assert_eq!(result["terminalsTruncated"], true);
    assert_eq!(result["orcaTotal"]["memoryBytes"], 610);
    assert!(!result.to_string().contains("private fixture history"));
    server.join().unwrap();
    assert!(
        !fixture
            .run(&["resources", "--limit", "0"], "")
            .status
            .success()
    );
    let schema = json_output(&fixture.run(&["schema", "resources"], ""));
    assert_eq!(schema["title"], "ResourceSnapshot");
}

#[test]
fn worktree_process_inventory_omits_prompts_and_preserves_incomplete_scope() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(1, |_, request| {
        assert_eq!(request["method"], "worktree.ps");
        assert_eq!(request["params"]["limit"], 1);
        json!({"worktrees":[{"worktreeId":"fixture","path":"/fixture","hasAttachedPty":true,"liveTerminalCount":2,"terminalVerdict":"live","preview":"private fixture","agents":[{"prompt":"private fixture"}]}],"totalCount":5,"truncated":true,"hostScope":{"hostIds":["local"],"omittedHostIds":["remote"]}})
    });
    let result = json_output(&fixture.run(&["worktrees", "ps", "--limit", "1"], ""));
    assert_eq!(result["worktrees"][0]["liveTerminalCount"], 2);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["hostScope"]["omittedHostIds"], json!(["remote"]));
    assert!(!result.to_string().contains("private fixture"));
    server.join().unwrap();
}

#[test]
fn terminal_stop_refuses_a_changed_incarnation_before_any_stop() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(1, |_, request| {
        assert_eq!(request["method"], "terminal.show");
        json!({"terminal":{"handle":"term-fixture","ptyId":"new-pty","worktreeId":"tree-fixture","incarnationId":"new-inc"}})
    });
    let output = fixture.run(
        &[
            "terminals",
            "stop",
            "term-fixture",
            "--expected-incarnation",
            "old-inc",
        ],
        "",
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("incarnation changed")
    );
    server.join().unwrap();
}

#[test]
fn cross_project_launch_requires_explicit_target_and_preserves_native_retry_receipt() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    for target in ["", " ", "active", "current", "all", "*"] {
        let output = fixture.run(&["sessions", "create", "--worktree", target], "");
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("explicit target worktree"));
    }
    for args in [
        vec!["sessions", "create"],
        vec![
            "sessions",
            "create",
            "--worktree",
            "path:/fixture",
            "--project",
            "other",
        ],
        vec![
            "sessions",
            "create",
            "--project",
            "other",
            "--retry-launch",
            "launch-fixture",
        ],
    ] {
        let output = fixture.run(&args, "");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let executable = fixture.data.join("orca-fixture");
    std::fs::write(&executable, "#!/usr/bin/env python3\nimport json,os,pathlib,sys\npathlib.Path(os.environ['ORCA_USER_DATA_PATH'],'argv.json').write_text(json.dumps(sys.argv[1:]))\nprint(json.dumps({'ok':True,'result':{'outcome':{'kind':'terminal'},'receipt':{'operationId':'launch-fixture','detail':'accepted fixture'}}}))\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prompt = "Literal $(do-not-run) `false`\nWork only in the selected project.";
    let mut child = fixture
        .command(&[
            "sessions",
            "create",
            "--worktree",
            "path:/fixture/project",
            "--agent",
            "codex",
            "--retry-launch",
            "launch-fixture",
            "--prompt",
            "-",
        ])
        .env("ORCA_BRIDGE_ORCA_CLI", &executable)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    let result = json_output(&child.wait_with_output().unwrap());
    assert_eq!(result["receipt"]["operationId"], "launch-fixture");
    assert_eq!(result["outcome"]["kind"], "terminal");
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(fixture.data.join("argv.json")).unwrap()).unwrap();
    assert_eq!(
        args,
        vec![
            "agent",
            "launch",
            "--agent",
            "codex",
            "--worktree",
            "path:/fixture/project",
            "--prompt",
            prompt,
            "--retry-launch",
            "launch-fixture",
            "--json"
        ]
    );
}

#[test]
fn session_renaming_is_searchable_reversible_and_does_not_change_orca_history() {
    let fixture = Fixture::new();
    let source = std::fs::read(fixture.data.join("ai-vault/session-search.sqlite")).unwrap();
    let renamed = json_output(&fixture.run(
        &[
            "sessions",
            "rename",
            "local:codex:alpha",
            "My launch plan",
            "--json",
        ],
        "",
    ));
    assert_eq!(renamed["title"], "My launch plan");
    assert_eq!(renamed["originalTitle"], "Session bridge design");
    let matches = json_output(&fixture.run(&["search", "launch plan", "--json"], ""));
    assert_eq!(matches[0]["id"], "local:codex:alpha");
    let old = json_output(&fixture.run(&["search", "Session bridge design", "--json"], ""));
    assert_eq!(old[0]["id"], "local:codex:alpha");
    assert_eq!(
        std::fs::read(fixture.data.join("ai-vault/session-search.sqlite")).unwrap(),
        source
    );
    let reset = json_output(&fixture.run(
        &["sessions", "rename", "local:codex:alpha", "", "--json"],
        "",
    ));
    assert_eq!(reset["title"], "Session bridge design");
    assert!(reset.get("originalTitle").is_none());
}

#[test]
fn context_injection_accepts_literal_stdin_through_both_send_commands() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let executable = fixture.data.join("fake-orca");
    std::fs::write(&executable,"#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$ORCA_USER_DATA_PATH/argv\"\nprintf '%s\\n' '{\"ok\":true,\"result\":{\"sent\":true}}'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let text = "context $(literal) `data`\nsecond line";
    for group in ["terminals", "sessions"] {
        let mut child = fixture
            .command(&[
                group,
                "send",
                "term-fixture",
                "--text",
                "-",
                "--enter",
                "--json",
            ])
            .env("ORCA_BRIDGE_ORCA_CLI", &executable)
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        assert_eq!(
            json_output(&child.wait_with_output().unwrap())["sent"],
            true
        );
        let bytes = std::fs::read(fixture.data.join("argv")).unwrap();
        let args = bytes
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "terminal",
                "send",
                "--terminal",
                "term-fixture",
                "--text",
                text,
                "--enter",
                "--json"
            ]
        );
    }
}

impl Fixture {
    fn fake_cli(&self, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.data.join("native-cli-fixture");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

#[test]
fn both_send_commands_observe_and_replay_the_same_prompt_without_resubmitting() {
    let fixture = Fixture::new();
    let cli = fixture.fake_cli("printf x >> \"$ORCA_USER_DATA_PATH/calls\"\nprintf '%s\\0' \"$@\" > \"$ORCA_USER_DATA_PATH/argv\"\nprintf '%s\\n' '{\"ok\":true,\"result\":{\"receipt\":{\"requestId\":\"request-fixture\",\"submissionStatus\":\"input-accepted\"},\"observed\":false}}'");
    for group in ["terminals", "sessions"] {
        let result = fixture
            .command(&[
                group,
                "send",
                "term-fixture",
                "--text",
                "literal $(never-run)",
                "--enter",
                "--wait-submit",
                "2",
                "--retry-request",
                "request-fixture",
            ])
            .env("ORCA_BRIDGE_ORCA_CLI", &cli)
            .output()
            .unwrap();
        let value = json_output(&result);
        assert_eq!(value["receipt"]["submissionStatus"], "input-accepted");
        assert_eq!(value["observed"], false);
        let bytes = std::fs::read(fixture.data.join("argv")).unwrap();
        let args = bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| std::str::from_utf8(part).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "terminal",
                "send",
                "--terminal",
                "term-fixture",
                "--text",
                "literal $(never-run)",
                "--enter",
                "--wait-submit",
                "2",
                "--retry-request",
                "request-fixture",
                "--json"
            ]
        );
    }
    assert_eq!(std::fs::read(fixture.data.join("calls")).unwrap(), b"xx");
    let output = fixture.run(
        &["terminals", "send", "term-fixture", "--wait-submit", "2"],
        "",
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn structured_errors_preserve_exit_codes_recovery_ids_and_clean_stdout() {
    let fixture = Fixture::new();
    let invalid = fixture.run(&["--json-errors", "sessions", "create"], "");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    let error: Value = serde_json::from_slice(&invalid.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_arguments");
    assert_eq!(error["error"]["outcome"], "not-started");
    let cli = fixture.fake_cli("printf '%s\\n' '{\"ok\":false,\"operationId\":\"operation-fixture\",\"error\":{\"code\":\"fixture_rejected\",\"message\":\"Rejected\"},\"recovery\":{\"retryRequestId\":\"request-fixture\"},\"authToken\":\"private-fixture\"}'\nexit 1");
    let output = fixture
        .command(&["--json-errors", "orca", "--", "fixture"])
        .env("ORCA_BRIDGE_ORCA_CLI", &cli)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["schemaVersion"], 1);
    assert_eq!(error["error"]["code"], "native_error");
    assert_eq!(error["error"]["nativeCode"], "fixture_rejected");
    assert_eq!(error["error"]["outcome"], "unknown");
    assert_eq!(error["error"]["operationId"], "operation-fixture");
    assert_eq!(error["error"]["retryRequestId"], "request-fixture");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-fixture"));
}

#[test]
fn native_prompt_recovery_id_is_distinct_from_rpc_correlation_and_hides_original_command() {
    let fixture = Fixture::new();
    let cli = fixture.fake_cli("printf '%s\\n' '{\"ok\":false,\"id\":\"rpc-correlation\",\"error\":{\"code\":\"runtime_unavailable\",\"message\":\"Reply lost\",\"data\":{\"orchestrationRequestId\":\"request-fixture\",\"originalCommand\":\"private original prompt\",\"deliveryOutcome\":\"unknown\"}}}'; exit 1");
    let result = fixture
        .command(&[
            "--json-errors",
            "terminals",
            "send",
            "term-fixture",
            "--text",
            "fixture",
            "--enter",
        ])
        .env("ORCA_BRIDGE_ORCA_CLI", &cli)
        .output()
        .unwrap();
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(error["error"]["retryRequestId"], "request-fixture");
    assert_eq!(error["error"]["outcome"], "unknown");
    assert!(!error.to_string().contains("private original prompt"));
    assert!(!error.to_string().contains("rpc-correlation"));
}

#[test]
fn delegated_cli_deadline_and_output_cap_never_retry_or_leak_output() {
    let fixture = Fixture::new();
    let cli = fixture.fake_cli("printf x >> \"$ORCA_USER_DATA_PATH/calls\"\nsleep 10");
    let start = std::time::Instant::now();
    let output = fixture
        .command(&[
            "--json-errors",
            "--command-timeout",
            "1",
            "orca",
            "--",
            "fixture",
        ])
        .env("ORCA_BRIDGE_ORCA_CLI", &cli)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "command_timeout");
    assert_eq!(error["error"]["outcome"], "unknown");
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(std::fs::read(fixture.data.join("calls")).unwrap(), b"x");
    let cli = fixture.fake_cli("printf private-fixture-output; printf private-fixture-output >&2");
    let output = fixture
        .command(&[
            "--json-errors",
            "--max-output-bytes",
            "5",
            "orca",
            "--",
            "fixture",
        ])
        .env("ORCA_BRIDGE_ORCA_CLI", &cli)
        .output()
        .unwrap();
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "output_limit");
    assert_eq!(error["error"]["outcome"], "unknown");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-fixture-output"));
}

#[test]
fn field_selection_is_compact_and_rejected_before_mutating_commands() {
    let fixture = Fixture::new();
    let rows = json_lines(&fixture.run(
        &["recent", "--fields", "title", "--jsonl", "--limit", "1"],
        "",
    ));
    assert_eq!(rows, vec![json!({"title":"Réparer café"})]);
    let result = json_output(&fixture.run(
        &["sessions", "messages", "alpha", "--fields", "messages"],
        "",
    ));
    assert_eq!(result["messages"], json!([]));
    assert_eq!(result["sourceTruncated"], false);
    assert_eq!(result["schemaVersion"], Value::Null);
    let cli = fixture.fake_cli("touch \"$ORCA_USER_DATA_PATH/should-not-run\"");
    let output = fixture
        .command(&[
            "--json-errors",
            "--fields",
            "receipt",
            "terminals",
            "send",
            "term-fixture",
            "--text",
            "fixture",
            "--enter",
        ])
        .env("ORCA_BRIDGE_ORCA_CLI", &cli)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["outcome"], "not-started");
    assert!(!fixture.data.join("should-not-run").exists());
}

#[test]
fn completions_and_new_schemas_are_offline_and_do_not_need_home() {
    let fixture = Fixture::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let output = fixture
            .command(&["completions", shell])
            .env_remove("HOME")
            .output()
            .unwrap();
        succeeded(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("orca-bridge"));
    }
    for entity in [
        "resources",
        "worktree-usage",
        "removal-plan",
        "removal-receipt",
        "error",
    ] {
        let output = fixture
            .command(&["schema", entity])
            .env_remove("HOME")
            .output()
            .unwrap();
        let schema = json_output(&output);
        assert!(schema.get("properties").is_some());
    }
}

#[test]
fn skills_routing_is_directive_machine_readable_and_available_offline() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&["skills", "routing", "--json"])
        .env_remove("HOME")
        .output()
        .unwrap();
    let guide = json_output(&output);
    assert_eq!(guide["routing"]["explicitTargetRequired"], true);
    assert_eq!(guide["routing"]["silentLocalFallbackAllowed"], false);
    assert_eq!(
        guide["routing"]["inProcessSubagentIsCrossProjectHandoff"],
        false
    );
    assert_eq!(
        guide["routing"]["ambiguousLaunch"]["freshDuplicateAllowed"],
        false
    );
    assert!(
        guide["markdown"]
            .as_str()
            .unwrap()
            .contains("Handoff template")
    );
    let skill = json_output(&fixture.run(&["skills", "--json"], ""));
    assert_eq!(skill["guides"], json!(["routing"]));
    assert!(
        skill["markdown"]
            .as_str()
            .unwrap()
            .contains("Do not silently edit the target project here")
    );
}

#[test]
fn completion_generation_treats_a_closed_downstream_as_success() {
    let fixture = Fixture::new();
    let mut child = fixture.command(&["completions", "zsh"]).spawn().unwrap();
    drop(child.stdout.take());
    succeeded(&child.wait_with_output().unwrap());
}

#[test]
fn resources_repository_filter_and_cpu_sort_apply_before_limit_with_global_totals() {
    let fixture = Fixture::new();
    let server = fixture.rpc_server(5, |index, request| match index {
        0 => { assert_eq!(request["method"],"repo.show"); assert_eq!(request["params"]["repo"],"name:chosen"); json!({"repo":{"id":"repo-chosen"}}) },
        1 | 4 => json!({"runtimeId":"fixture"}),
        2 => json!({"collectedAt":1,"processMemoryMetric":"rss","app":{},"host":{},"totalMemory":900,"worktrees":[{"worktreeId":"tree-other","repoId":"repo-other","sessions":[{"sessionId":"pty-other","memory":900,"cpu":99}]},{"worktreeId":"tree-chosen","repoId":"repo-chosen","sessions":[{"sessionId":"pty-slow","memory":200,"cpu":1},{"sessionId":"pty-busy","memory":100,"cpu":10}]}]}),
        _ => json!({"terminals":[]})
    });
    let report = json_output(&fixture.run(
        &[
            "resources",
            "--repo",
            "name:chosen",
            "--sort",
            "cpu",
            "--limit",
            "1",
        ],
        "",
    ));
    assert_eq!(report["terminals"][0]["ptyId"], "pty-busy");
    assert_eq!(report["selection"]["repoId"], "repo-chosen");
    assert_eq!(report["terminalCount"], 2);
    assert_eq!(report["terminalsTruncated"], true);
    assert_eq!(report["orcaTotal"]["memoryBytes"], 900);
    server.join().unwrap();
}
