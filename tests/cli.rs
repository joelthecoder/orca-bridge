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
