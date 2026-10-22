//! Read-only history adapter. Provider conversation IDs are never terminal handles.
use crate::{
    config::{Config, readonly_db},
    model::Session,
};
use anyhow::{Context, Result, bail};
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::UNIX_EPOCH,
};

const TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// A reversible, URL-safe identifier. Separators inside components are escaped.
pub fn session_id(agent: &str, provider_id: &str) -> String {
    fn component(s: &str) -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }
    format!("local:{}:{}", component(agent), component(provider_id))
}

pub fn load_sessions(config: &Config) -> Result<Vec<Session>> {
    let db = readonly_db(&config.search_db())?;
    let mut statement = db.prepare("SELECT agent,session_id,title,cwd,branch,created_at,updated_at,message_count,file_path,codex_home FROM sessions")
        .context("Unsupported Orca session index schema")?;
    let mut sessions = statement
        .query_map([], |row| {
            let agent: String = row.get(0)?;
            let provider_id: String = row.get(1)?;
            Ok(Session {
                id: session_id(&agent, &provider_id),
                host: "local".into(),
                agent,
                session_id: provider_id,
                title: row.get(2)?,
                original_title: None,
                cwd: row.get(3)?,
                branch: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                message_count: row.get(7)?,
                file_path: row.get(8)?,
                codex_home: row.get(9)?,
                model: None,
                total_tokens: None,
                usage: None,
                score: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // This is a partial, disposable cache. Invalid/missing entries must not hide history.
    // serde_json's reader consumes bytes individually. Passing a bare File here made
    // a 484 KiB cache cost ~190 ms in read syscalls on macOS. Read once, then parse.
    if let Ok(bytes) = std::fs::read(config.parse_cache())
        && let Ok(cache) = serde_json::from_slice::<Value>(&bytes)
        && let Some(entries) = cache.get("entries").and_then(Value::as_array)
    {
        let indices: HashMap<_, _> = sessions
            .iter()
            .enumerate()
            .map(|(i, s)| ((s.agent.clone(), s.session_id.clone()), i))
            .collect();
        for entry in entries {
            let Some(cached) = entry.get(1) else { continue };
            let Some(session) = cached.get("session") else {
                continue;
            };
            let (Some(agent), Some(id)) =
                (session["agent"].as_str(), session["sessionId"].as_str())
            else {
                continue;
            };
            let Some(&i) = indices.get(&(agent.to_owned(), id.to_owned())) else {
                continue;
            };
            let Some(path) = sessions[i].file_path.as_deref() else {
                continue;
            };
            if session["filePath"].as_str() != Some(path) || !cache_current(cached, Path::new(path))
            {
                continue;
            }
            // This index is local. Never let a cache entry from another host relabel it.
            if session
                .get("executionHostId")
                .and_then(Value::as_str)
                .is_some_and(|host| host != "local")
            {
                continue;
            }
            sessions[i].model = session["model"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            sessions[i].total_tokens = session["totalTokens"].as_u64();
        }
    }
    let labels_path = config.state_dir.join("session-labels.sqlite");
    if labels_path.is_file() {
        let labels = readonly_db(&labels_path)?;
        let mut statement = labels.prepare("SELECT session_id,title FROM labels")?;
        let titles = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        for session in &mut sessions {
            if let Some(title) = titles.get(&session.id) {
                session.original_title = Some(std::mem::replace(&mut session.title, title.clone()));
            }
        }
    }
    Ok(sessions)
}

/// A UI display title owned by the bridge. Never rewrites provider history or Orca DBs.
pub fn rename(config: &Config, id: &str, title: &str) -> Result<Session> {
    anyhow::ensure!(
        title.len() <= 4096 && !title.chars().any(char::is_control),
        "Title must be at most 4096 bytes without control characters"
    );
    let session = get_session(config, id)?;
    std::fs::create_dir_all(&config.state_dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&config.state_dir, std::fs::Permissions::from_mode(0o700))?;
    let path = config.state_dir.join("session-labels.sqlite");
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        anyhow::ensure!(
            metadata.is_file(),
            "Session label database must be a regular file"
        );
    }
    let db = rusqlite::Connection::open(&path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS labels(session_id TEXT PRIMARY KEY,title TEXT NOT NULL)",
        [],
    )?;
    if title.is_empty() {
        db.execute("DELETE FROM labels WHERE session_id=?1", [&session.id])?;
    } else {
        db.execute("INSERT INTO labels VALUES(?1,?2) ON CONFLICT(session_id) DO UPDATE SET title=excluded.title",rusqlite::params![session.id,title])?;
    }
    get_session(config, &session.id)
}

fn cache_current(entry: &Value, path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    let Some(size) = entry["sizeBytes"].as_u64() else {
        return false;
    };
    let Some(mtime) = entry["mtimeMs"].as_f64() else {
        return false;
    };
    let Ok(modified) = metadata.modified().and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) else {
        return false;
    };
    size == metadata.len() && (modified.as_secs_f64() * 1000.0 - mtime).abs() <= 1.0
}

pub fn get_session(config: &Config, id: &str) -> Result<Session> {
    if id.trim().is_empty() {
        bail!("Session ID must not be empty")
    }
    let sessions = load_sessions(config)?;
    if let Some(session) = sessions.iter().find(|s| s.id == id) {
        return Ok(session.clone());
    }
    // Raw provider IDs are accepted only when unique across providers.
    let mut matches = sessions
        .into_iter()
        .filter(|s| s.id.starts_with(id) || s.session_id.starts_with(id));
    let Some(session) = matches.next() else {
        bail!("No session matches {id:?}")
    };
    if matches.next().is_some() {
        bail!("Ambiguous session {id:?}; use its complete durable ID")
    }
    Ok(session)
}

fn timestamp(session: &Session) -> i64 {
    session
        .updated_at
        .as_deref()
        .or(session.created_at.as_deref())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|date| date.timestamp_millis())
        .unwrap_or(i64::MIN)
}

pub fn recent(mut sessions: Vec<Session>, limit: usize) -> Vec<Session> {
    sessions.sort_by(|a, b| {
        timestamp(b)
            .cmp(&timestamp(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    sessions.truncate(limit);
    sessions
}

/// Fuzzy match each query word across fields, with conservative adjacent-typo fallback.
pub fn search(sessions: Vec<Session>, query: &str, limit: usize) -> Vec<Session> {
    if limit == 0 {
        return Vec::new();
    }
    if query.trim().is_empty() {
        return recent(sessions, limit);
    }
    let terms: Vec<_> = query
        .split_whitespace()
        .map(|s| {
            (
                s.to_lowercase().chars().collect::<Vec<_>>(),
                Atom::new(
                    s,
                    CaseMatching::Smart,
                    Normalization::Smart,
                    AtomKind::Fuzzy,
                    false,
                ),
            )
        })
        .collect();
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buffer = Vec::new();
    let mut matches: Vec<_> = sessions
        .into_iter()
        .filter_map(|mut session| {
            let fields = [
                session.title.as_str(),
                session.original_title.as_deref().unwrap_or(""),
                session.cwd.as_deref().unwrap_or(""),
                session.branch.as_deref().unwrap_or(""),
                session.model.as_deref().unwrap_or(""),
                session.agent.as_str(),
            ];
            let mut total = 0;
            for (term, atom) in &terms {
                let best = fields
                    .iter()
                    .enumerate()
                    .filter_map(|(i, field)| {
                        let weight = if i <= 1 { 3 } else { 2 };
                        atom.score(Utf32Str::new(field, &mut buffer), &mut matcher)
                            .map(|s| u32::from(s) * weight + 100)
                            .or_else(|| typo_match(term, field).then_some(25 * weight))
                    })
                    .max()?;
                total += best;
            }
            session.score = Some(total);
            Some(session)
        })
        .collect();
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| timestamp(b).cmp(&timestamp(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    matches.truncate(limit);
    matches
}

fn typo_match(needle: &[char], field: &str) -> bool {
    if !(4..=64).contains(&needle.len()) {
        return false;
    }
    field.split(|c: char| !c.is_alphanumeric()).any(|word| {
        if word.is_ascii() && word.len() != needle.len() {
            return false;
        }
        let word: Vec<char> = word.chars().flat_map(char::to_lowercase).collect();
        if word.len() != needle.len() {
            return false;
        }
        let mut differences = needle
            .iter()
            .zip(&word)
            .enumerate()
            .filter_map(|(i, (a, b))| (a != b).then_some(i))
            .take(3);
        match (differences.next(), differences.next(), differences.next()) {
            (Some(_), None, None) => true,
            (Some(a), Some(b), None) => b == a + 1 && needle[a] == word[b] && needle[b] == word[a],
            _ => false,
        }
    })
}

pub fn profile_domain(path: &Path, domain: &str) -> Result<Value> {
    let db = readonly_db(path)?;
    let payload: String = db
        .query_row(
            "SELECT payload FROM profile_state_documents WHERE domain=?1",
            [domain],
            |row| row.get(0),
        )
        .with_context(|| format!("Cannot read Orca profile domain {domain:?}"))?;
    serde_json::from_str(&payload).context("Invalid Orca profile JSON")
}

/// Read only a bounded transcript tail. Codex cumulative counters remain complete;
/// Claude's per-message counters are explicitly partial when the tail is truncated.
pub fn usage(session: &Session) -> Result<Value> {
    let Some(path) = session.file_path.as_deref() else {
        return Ok(unknown("No transcript path"));
    };
    if !matches!(session.agent.as_str(), "codex" | "claude") {
        return Ok(unknown("Unsupported provider transcript"));
    }
    let mut file = File::open(path).with_context(|| format!("Cannot read transcript {path}"))?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    let mut lines = bytes.split(|b| *b == b'\n');
    if start > 0 {
        lines.next();
    }
    let mut codex = None;
    let mut claude: HashMap<String, Value> = HashMap::new();
    for line in lines {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if session.agent == "codex"
            && record["type"] == "event_msg"
            && record["payload"]["type"] == "token_count"
        {
            let info = &record["payload"]["info"];
            if info["total_token_usage"]
                .as_object()
                .is_some_and(|obj| obj.values().any(|v| v.as_u64().is_some()))
            {
                codex = Some(
                    json!({"status":"known", "source":"codex.token_count", "scope":"session", "total":info["total_token_usage"], "last":info["last_token_usage"], "modelContextWindow":info["model_context_window"], "tailTruncated":start > 0}),
                );
            }
        } else if session.agent == "claude" && record["type"] == "assistant" {
            let message = &record["message"];
            if record["isSynthetic"] == true
                || record["synthetic"] == true
                || message["model"]
                    .as_str()
                    .is_some_and(|m| m.contains("synthetic"))
            {
                continue;
            }
            let Some(id) = message["id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let Some(counters) = message["usage"].as_object() else {
                continue;
            };
            let keys = [
                "input_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
                "output_tokens",
            ];
            let total: u64 = keys
                .iter()
                .filter_map(|key| counters.get(*key).and_then(Value::as_u64))
                .fold(0, u64::saturating_add);
            if total == 0 {
                continue;
            }
            let replace = claude.get(id).is_none_or(|old| {
                keys.iter()
                    .filter_map(|key| old[*key].as_u64())
                    .fold(0, u64::saturating_add)
                    <= total
            });
            if replace {
                claude.insert(id.to_owned(), Value::Object(counters.clone()));
            }
        }
    }
    if session.agent == "codex" {
        return Ok(
            codex.unwrap_or_else(|| unknown("No token_count counters in bounded transcript tail"))
        );
    }
    if claude.is_empty() {
        return Ok(unknown(
            "No attributable non-synthetic provider counters in bounded transcript tail",
        ));
    }
    let mut total = serde_json::Map::new();
    let mut observed_total = serde_json::Map::new();
    let mut counter_coverage = serde_json::Map::new();
    for key in [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "output_tokens",
    ] {
        let known: Vec<_> = claude.values().filter_map(|v| v[key].as_u64()).collect();
        let count = known.len();
        let observed = if known.is_empty() {
            Value::Null
        } else {
            json!(known.into_iter().fold(0, u64::saturating_add))
        };
        counter_coverage.insert(key.into(), json!(count));
        total.insert(
            key.into(),
            if count == claude.len() {
                observed.clone()
            } else {
                Value::Null
            },
        );
        observed_total.insert(key.into(), observed);
    }
    Ok(
        json!({"status":"known", "source":"claude.message.usage", "scope":if start > 0 { "tail" } else { "session" }, "total":total, "observedTotal":observed_total, "counterCoverage":counter_coverage, "providerMessages":claude.len(), "tailTruncated":start > 0}),
    )
}

fn unknown(reason: &str) -> Value {
    json!({"status":"unknown", "reason":reason})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, updated: &str) -> Session {
        Session {
            id: session_id("codex", id),
            host: "local".into(),
            agent: "codex".into(),
            session_id: id.into(),
            title: title.into(),
            original_title: None,
            cwd: Some("/fixture/project".into()),
            branch: Some("main".into()),
            model: Some("gpt-example".into()),
            created_at: None,
            updated_at: Some(updated.into()),
            message_count: 1,
            file_path: None,
            codex_home: None,
            total_tokens: None,
            usage: None,
            score: None,
        }
    }

    fn config(root: &Path) -> Config {
        Config {
            data_dir: root.to_path_buf(),
            state_dir: root.join("state"),
            orca_cli: root.join("orca"),
            profile_db_override: None,
        }
    }

    fn create_index(config: &Config) {
        std::fs::create_dir_all(config.search_db().parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(config.search_db()).unwrap();
        db.execute_batch("CREATE TABLE sessions(agent TEXT, session_id TEXT,title TEXT,cwd TEXT,branch TEXT,created_at TEXT,updated_at TEXT,message_count INTEGER,file_path TEXT,codex_home TEXT)").unwrap();
    }

    fn insert(config: &Config, id: &str, path: &Path) {
        let db = rusqlite::Connection::open(config.search_db()).unwrap();
        db.execute("INSERT INTO sessions VALUES('codex',?1,'Fixture','/fixture',NULL,NULL,'2026-10-21T01:00:00Z',1,?2,NULL)", rusqlite::params![id,path.to_str().unwrap()]).unwrap();
    }

    #[test]
    fn durable_id_escapes_delimiters_and_unicode() {
        assert_eq!(
            session_id("c:odex", "a/b Ω"),
            "local:c%3Aodex:a%2Fb%20%CE%A9"
        );
        assert_ne!(session_id("a:b", "c"), session_id("a", "b:c"));
    }

    #[test]
    fn fuzzy_ranks_exact_above_typo_and_matches_unicode_across_fields() {
        let mut unicode = session("unicode", "Réparer café", "2026-10-21T00:00:00Z");
        unicode.branch = Some("feature/東京".into());
        let result = search(vec![unicode], "cafe 東京", 10);
        assert_eq!(result.len(), 1);
        let result = search(
            vec![
                session("typo", "seasions", "2026-10-21T00:00:00Z"),
                session("exact", "sessions", "2026-01-01T00:00:00Z"),
            ],
            "sessions",
            10,
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].session_id, "exact");
        assert_eq!(
            search(
                vec![session("swap", "sessions", "2026-01-01T00:00:00Z")],
                "sessinos",
                10
            )
            .len(),
            1
        );
        assert_eq!(
            search(
                vec![session("swap", "sessions", "2026-01-01T00:00:00Z")],
                "ses sions",
                10
            )
            .len(),
            1
        );
    }

    #[test]
    fn recency_compares_instants_not_timezone_strings() {
        let result = recent(
            vec![
                session("old", "same", "2026-10-21T01:30:00+02:00"),
                session("new", "same", "2026-10-21T00:00:00Z"),
            ],
            1,
        );
        assert_eq!(result[0].session_id, "new");
        assert!(recent(result, 0).is_empty());
    }

    #[test]
    fn index_is_preserved_and_session_prefix_ambiguity_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        create_index(&config);
        insert(&config, "abc-one", Path::new("/fixture/missing"));
        insert(&config, "abc-two", Path::new("/fixture/missing"));
        let before = std::fs::read(config.search_db()).unwrap();
        assert_eq!(load_sessions(&config).unwrap().len(), 2);
        assert!(
            get_session(&config, "abc")
                .unwrap_err()
                .to_string()
                .contains("Ambiguous")
        );
        assert_eq!(
            get_session(&config, "local:codex:abc-one")
                .unwrap()
                .session_id,
            "abc-one"
        );
        assert_eq!(before, std::fs::read(config.search_db()).unwrap());
        assert!(!config.search_db().with_extension("sqlite-journal").exists());
        let db = readonly_db(&config.search_db()).unwrap();
        assert!(db.execute("DELETE FROM sessions", []).is_err());
    }

    #[test]
    fn partial_and_stale_cache_never_hides_history_or_supplies_stale_model() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        create_index(&config);
        let path = directory.path().join("fixture.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        insert(&config, "cached", &path);
        insert(&config, "uncached", Path::new("/fixture/missing"));
        let metadata = path.metadata().unwrap();
        let entry = json!({"mtimeMs":metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()*1000.0,"sizeBytes":metadata.len(),"session":{"agent":"codex","sessionId":"cached","executionHostId":"local","filePath":path,"model":"gpt-fixture","totalTokens":500}});
        std::fs::write(
            config.parse_cache(),
            json!({"entries":[["fixture",entry],["bad",null]]}).to_string(),
        )
        .unwrap();
        let loaded = load_sessions(&config).unwrap();
        assert_eq!(loaded[0].model.as_deref(), Some("gpt-fixture"));
        assert_eq!(loaded[0].total_tokens, Some(500));
        assert!(loaded[1].model.is_none());
        std::fs::write(&path, "changed\n").unwrap();
        assert!(load_sessions(&config).unwrap()[0].model.is_none());
        std::fs::write(config.parse_cache(), "malformed").unwrap();
        assert_eq!(load_sessions(&config).unwrap().len(), 2);
    }

    fn transcript_usage(agent: &str, contents: &str) -> Value {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transcript.jsonl");
        std::fs::write(&path, contents).unwrap();
        let mut s = session("fixture", "Fixture", "2026-01-01T00:00:00Z");
        s.agent = agent.into();
        s.file_path = Some(path.to_str().unwrap().into());
        usage(&s).unwrap()
    }

    #[test]
    fn codex_uses_latest_cumulative_counters_and_tolerates_partial_records() {
        let result = transcript_usage(
            "codex",
            concat!(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":50,\"output_tokens\":20,\"reasoning_output_tokens\":10,\"total_tokens\":120},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":200000}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":200,\"cached_input_tokens\":100,\"output_tokens\":30,\"total_tokens\":230}}}}\n",
                "{\"partial\":"
            ),
        );
        assert_eq!(result["total"]["total_tokens"], 230);
        assert_eq!(result["total"]["cached_input_tokens"], 100);
        assert_eq!(result["scope"], "session");
    }

    #[test]
    fn claude_counts_provider_messages_once_and_excludes_synthetic_usage() {
        let result = transcript_usage(
            "claude",
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_a\",\"model\":\"claude-fixture\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":20}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_a\",\"model\":\"claude-fixture\",\"usage\":{\"input_tokens\":10,\"output_tokens\":8,\"cache_read_input_tokens\":20}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_b\",\"model\":\"claude-fixture\",\"usage\":{\"input_tokens\":5,\"output_tokens\":3}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"synthetic\",\"model\":\"<synthetic>\",\"usage\":{\"input_tokens\":999,\"output_tokens\":999}}}\n"
            ),
        );
        assert_eq!(result["total"]["input_tokens"], 15);
        assert_eq!(result["total"]["output_tokens"], 11);
        assert_eq!(result["providerMessages"], 2);
        assert!(result["total"]["cache_creation_input_tokens"].is_null());
        assert!(result["total"]["cache_read_input_tokens"].is_null());
        assert_eq!(result["observedTotal"]["cache_read_input_tokens"], 20);
        assert_eq!(result["counterCoverage"]["cache_read_input_tokens"], 1);
        assert_eq!(
            transcript_usage(
                "claude",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"synthetic\",\"model\":\"<synthetic>\",\"usage\":{\"input_tokens\":0}}}\n"
            )["status"],
            "unknown"
        );
    }

    #[test]
    fn bounded_claude_tail_is_partial() {
        let mut contents = " ".repeat(TAIL_BYTES as usize + 10);
        contents.push('\n');
        contents.push_str("{\"type\":\"assistant\",\"message\":{\"id\":\"msg_a\",\"usage\":{\"input_tokens\":10}}}\n");
        let result = transcript_usage("claude", &contents);
        assert_eq!(result["scope"], "tail");
        assert_eq!(result["tailTruncated"], true);
    }

    #[test]
    fn profile_domain_returns_parsed_json_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE profile_state_documents(domain TEXT,payload TEXT);INSERT INTO profile_state_documents VALUES('projects','{\"items\":[]}');").unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            profile_domain(&path, "projects").unwrap(),
            json!({"items":[]})
        );
        assert_eq!(before, std::fs::read(path).unwrap());
    }
}
