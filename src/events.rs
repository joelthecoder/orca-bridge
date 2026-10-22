use crate::{config::Config, history, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub static STOP: AtomicBool = AtomicBool::new(false);

pub fn journal(config: &Config) -> Result<Connection> {
    std::fs::create_dir_all(&config.state_dir)?;
    std::fs::set_permissions(&config.state_dir, std::fs::Permissions::from_mode(0o700))?;
    let path = config.state_dir.join("events.sqlite");
    let db = Connection::open(&path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY AUTOINCREMENT, source_key TEXT UNIQUE, timestamp TEXT NOT NULL, topic TEXT NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL); CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
    Ok(db)
}

pub fn store(db: &Connection, event: &mut Value, source_key: Option<&str>) -> Result<bool> {
    let changed = db.execute(
        "INSERT OR IGNORE INTO events(source_key,timestamp,topic,kind,data) VALUES(?1,?2,?3,?4,?5)",
        params![
            source_key,
            event["timestamp"].as_str(),
            event["topic"].as_str(),
            event["kind"].as_str(),
            event["data"].to_string()
        ],
    )?;
    if changed != 0 {
        event["id"] = db.last_insert_rowid().into();
    }
    Ok(changed != 0)
}

pub fn replay(
    config: &Config,
    since: i64,
    limit: usize,
    topic: Option<&str>,
) -> Result<Vec<Value>> {
    let db = journal(config)?;
    let mut stmt = db.prepare("SELECT id,timestamp,topic,kind,data FROM events WHERE id>?1 AND (?2 IS NULL OR topic=?2) ORDER BY id LIMIT ?3")?;
    let rows = stmt.query_map(params![since,topic,limit as i64], |r| {
        let data: String = r.get(4)?;
        Ok(json!({"schemaVersion":1,"id":r.get::<_,i64>(0)?,"timestamp":r.get::<_,String>(1)?,"topic":r.get::<_,String>(2)?,"kind":r.get::<_,String>(3)?,"data":serde_json::from_str::<Value>(&data).unwrap_or(Value::Null)}))
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn status(config: &Config) -> Result<Value> {
    let db = journal(config)?;
    let (count, first, last): (i64, Option<i64>, Option<i64>) =
        db.query_row("SELECT count(*),min(id),max(id) FROM events", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    Ok(
        json!({"events":count,"oldestId":first,"newestId":last,"path":config.state_dir.join("events.sqlite")}),
    )
}

fn prune_db(db: &Connection, days: u32, count: usize) -> Result<usize> {
    ensure!(
        (1..=36500).contains(&days) && (1..=1_000_000).contains(&count),
        "Retention requires 1–36500 days and 1–1000000 events"
    );
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(i64::from(days)))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Ok(db.execute("DELETE FROM events WHERE timestamp < ?1 OR id < COALESCE((SELECT id FROM events ORDER BY id DESC LIMIT 1 OFFSET ?2),0)",params![cutoff,(count-1) as i64])?)
}

pub fn prune(config: &Config, days: u32, count: usize) -> Result<Value> {
    let removed = prune_db(&journal(config)?, days, count)?;
    Ok(json!({"removed":removed,"keepDays":days,"keepCount":count}))
}

fn event(topic: &str, kind: &str, data: Value) -> Value {
    json!({"schemaVersion":1,"timestamp":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"topic":topic,"kind":kind,"data":data})
}

#[derive(Default)]
pub struct Differ {
    previous: BTreeMap<String, Value>,
    initialized: bool,
}

impl Differ {
    pub fn update(&mut self, topic: &str, rows: Vec<(String, Value)>, initial: bool) -> Vec<Value> {
        let next: BTreeMap<_, _> = rows.into_iter().collect();
        let mut events = Vec::new();
        if !self.initialized {
            if initial {
                events.push(event(
                    topic,
                    "snapshot",
                    json!({"items":next.values().collect::<Vec<_>>()}),
                ));
            }
        } else {
            for (key, value) in &next {
                match self.previous.get(key) {
                    None => events.push(event(topic, "created", value.clone())),
                    Some(old) if old != value => {
                        events.push(event(topic, "updated", value.clone()))
                    }
                    _ => {}
                }
            }
            for (key, value) in &self.previous {
                if !next.contains_key(key) {
                    events.push(event(topic, "removed", value.clone()));
                }
            }
        }
        self.previous = next;
        self.initialized = true;
        events
    }
}

pub struct WatchOptions {
    pub topics: Vec<String>,
    pub interval: Duration,
    pub timeout: Option<Duration>,
    pub count: Option<usize>,
    pub initial: bool,
    pub record: bool,
    pub retention_days: u32,
    pub retention_count: usize,
}

pub fn watch(config: &Config, options: WatchOptions, out: &mut impl Write) -> Result<()> {
    ensure!(
        options.interval >= Duration::from_millis(100),
        "Minimum polling interval is 100 ms"
    );
    ensure!(
        !options.topics.is_empty()
            && options.topics.iter().all(|t| matches!(
                t.as_str(),
                "sessions" | "terminals" | "status" | "notifications" | "worktrees" | "projects"
            )),
        "Unknown watch topic"
    );
    let rpc = RpcClient::new(config).with_timeout(Duration::from_secs(3));
    let db = if options.record {
        Some(journal(config)?)
    } else {
        None
    };
    let mut diffs = BTreeMap::<String, Differ>::new();
    let mut failures = BTreeMap::<String, String>::new();
    let mut watermark = 0u64;
    let mut epoch: Option<String> = None;
    if let Some(db) = &db
        && let Ok(value) = db.query_row(
            "SELECT value FROM meta WHERE key='notification_cursor'",
            [],
            |r| r.get::<_, String>(0),
        )
    {
        let cursor: Value = serde_json::from_str(&value)?;
        watermark = cursor["seq"].as_u64().unwrap_or(0);
        epoch = cursor["epoch"].as_str().map(str::to_owned);
    }
    let start = Instant::now();
    let mut emitted = 0usize;
    let mut last_prune = Instant::now();
    if let Some(db) = &db {
        prune_db(db, options.retention_days, options.retention_count)?;
    }
    while !STOP.load(Ordering::Relaxed) && options.timeout.is_none_or(|d| start.elapsed() < d) {
        if last_prune.elapsed() >= Duration::from_secs(60) {
            if let Some(db) = &db {
                prune_db(db, options.retention_days, options.retention_count)?;
            }
            last_prune = Instant::now();
        }
        for topic in &options.topics {
            let result: Result<Vec<Value>> = (|| {
                if topic == "notifications" {
                    let mut params =
                        json!({"lastSeenSeq":watermark,"includeDesktopSuppressed":true});
                    if let Some(epoch) = &epoch {
                        params["epoch"] = epoch.clone().into();
                    }
                    let response = rpc.call("notifications.getMissedSince", params)?;
                    let next_epoch = response["epoch"].as_str().map(str::to_owned);
                    if next_epoch != epoch {
                        watermark = 0;
                    }
                    epoch = next_epoch;
                    let mut events = Vec::new();
                    for notification in response["notifications"].as_array().into_iter().flatten() {
                        let seq = notification["notificationSeq"].as_u64().unwrap_or(0);
                        watermark = watermark.max(seq);
                        let mut e = event(topic, "notification", notification.clone());
                        let key = format!(
                            "notifications:{}:{seq}",
                            epoch.as_deref().unwrap_or("unknown")
                        );
                        if let Some(db) = &db
                            && !store(db, &mut e, Some(&key))?
                        {
                            continue;
                        }
                        events.push(e);
                    }
                    if let Some(db) = &db {
                        db.execute("INSERT INTO meta(key,value) VALUES('notification_cursor',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[json!({"epoch":epoch,"seq":watermark}).to_string()])?;
                    }
                    return Ok(events);
                }
                let rows: Vec<(String, Value)> = match topic.as_str() {
                    "sessions" => history::load_sessions(config)?
                        .into_iter()
                        .map(|s| Ok((s.id.clone(), serde_json::to_value(s)?)))
                        .collect::<Result<_>>()?,
                    "terminals" => {
                        let r = rpc.call("terminal.list", json!({"includeVisualLayouts":false}))?;
                        r["terminals"]
                            .as_array()
                            .context("Missing terminal inventory")?
                            .iter()
                            .map(|t| {
                                let mut t = t.clone();
                                t.as_object_mut().map(|o| o.remove("preview"));
                                (t["handle"].as_str().unwrap_or_default().to_owned(), t)
                            })
                            .collect()
                    }
                    "status" => {
                        let r = rpc.call("terminal.list", json!({"includeVisualLayouts":false}))?;
                        let mut rows = Vec::new();
                        for t in r["terminals"]
                            .as_array()
                            .context("Missing terminal inventory")?
                        {
                            let handle = t["handle"].as_str().context("Terminal missing handle")?;
                            let mut r=rpc.call("terminal.agentStatus",json!({"terminal":handle,"expectedIncarnationId":t["incarnationId"]}))?["agentStatus"].clone();
                            r["worktreeId"] = t["worktreeId"].clone();
                            r["title"] = t["title"].clone();
                            r["incarnationId"] = t["incarnationId"].clone();
                            rows.push((handle.to_owned(), r));
                        }
                        rows
                    }
                    "worktrees" => {
                        let r = rpc.call("worktree.ps", json!({}))?;
                        r["worktrees"]
                            .as_array()
                            .context("Missing worktrees")?
                            .iter()
                            .map(|v| {
                                let mut v = v.clone();
                                v.as_object_mut().map(|o| o.remove("preview"));
                                (v["worktreeId"].as_str().unwrap_or_default().to_owned(), v)
                            })
                            .collect()
                    }
                    "projects" => {
                        let r = rpc.call("project.list", json!({}))?;
                        r["projects"]
                            .as_array()
                            .context("Missing projects")?
                            .iter()
                            .map(|v| {
                                let mut v = v.clone();
                                v.as_object_mut().map(|o| o.remove("repoIcon"));
                                (v["id"].as_str().unwrap_or_default().to_owned(), v)
                            })
                            .collect()
                    }
                    _ => anyhow::bail!("Unknown topic {topic}"),
                };
                Ok(diffs
                    .entry(topic.clone())
                    .or_default()
                    .update(topic, rows, options.initial))
            })();
            match result {
                Ok(events) => {
                    if failures.remove(topic).is_some() {
                        let mut recovered = event(topic, "reconnected", json!({"live":true}));
                        if let Some(db) = &db {
                            store(db, &mut recovered, None)?;
                        }
                        serde_json::to_writer(&mut *out, &recovered)?;
                        out.write_all(b"\n")?;
                        out.flush()?;
                        emitted += 1;
                        if options.count.is_some_and(|n| emitted >= n) {
                            return Ok(());
                        }
                    }
                    for mut e in events {
                        if topic != "notifications"
                            && let Some(db) = &db
                        {
                            store(db, &mut e, None)?;
                        }
                        serde_json::to_writer(&mut *out, &e)?;
                        out.write_all(b"\n")?;
                        out.flush()?;
                        emitted += 1;
                        if options.count.is_some_and(|n| emitted >= n) {
                            return Ok(());
                        }
                    }
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    if failures.get(topic) != Some(&message) {
                        failures.insert(topic.clone(), message.clone());
                        let mut e =
                            event(topic, "unavailable", json!({"live":false,"error":message}));
                        if let Some(db) = &db {
                            store(db, &mut e, None)?;
                        }
                        serde_json::to_writer(&mut *out, &e)?;
                        out.write_all(b"\n")?;
                        out.flush()?;
                        emitted += 1;
                        if options.count.is_some_and(|n| emitted >= n) {
                            return Ok(());
                        }
                    }
                }
            }
        }
        let pause = options.timeout.map_or(options.interval, |d| {
            options.interval.min(d.saturating_sub(start.elapsed()))
        });
        std::thread::sleep(pause);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_preserves_cursor_metadata_and_monotonic_ids() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::discover(
            Some(dir.path().join("orca")),
            Some(dir.path().join("state")),
        )
        .unwrap();
        let db = journal(&config).unwrap();
        db.execute(
            "INSERT INTO meta VALUES('notification_cursor','fixture-cursor')",
            [],
        )
        .unwrap();
        for i in 0..5 {
            let mut e = event("sessions", "updated", json!({"n":i}));
            if i == 0 {
                e["timestamp"] = json!("2000-01-01T00:00:00.000Z");
            }
            store(&db, &mut e, None).unwrap();
        }
        assert_eq!(prune_db(&db, 30, 2).unwrap(), 3);
        assert_eq!(status(&config).unwrap()["oldestId"], 4);
        let mut e = event("sessions", "updated", Value::Null);
        store(&db, &mut e, None).unwrap();
        assert_eq!(e["id"], 6);
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='notification_cursor'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "fixture-cursor"
        );
        assert!(prune_db(&db, 0, 2).is_err());
    }
    #[test]
    fn differences_preserve_created_updated_and_removed() {
        let mut d = Differ::default();
        assert!(
            d.update(
                "sessions",
                vec![("a".into(), json!({"title":"first"}))],
                false
            )
            .is_empty()
        );
        let e = d.update(
            "sessions",
            vec![
                ("a".into(), json!({"title":"changed"})),
                ("b".into(), json!({})),
            ],
            false,
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["kind"], "updated");
        assert_eq!(e[1]["kind"], "created");
        let e = d.update("sessions", vec![], false);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["kind"], "removed");
    }
    #[test]
    fn journal_deduplicates_source_and_replays_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::discover(
            Some(temp.path().join("orca")),
            Some(temp.path().join("bridge")),
        )
        .unwrap();
        let db = journal(&config).unwrap();
        let mut e = event("notifications", "notification", json!({"title":"fixture"}));
        assert!(store(&db, &mut e, Some("epoch:1")).unwrap());
        assert!(!store(&db, &mut e, Some("epoch:1")).unwrap());
        let replay = replay(&config, 0, 10, None).unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0]["data"]["title"], "fixture");
    }
}
