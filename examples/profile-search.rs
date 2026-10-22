//! Read-only timing of the direct search path. No session contents are printed.
use anyhow::Result;
use orca_bridge::{
    config::{Config, readonly_db},
    history,
};
use serde_json::json;
use std::time::Instant;

fn main() -> Result<()> {
    let config = Config::discover(None, None)?;
    let start = Instant::now();
    let db = readonly_db(&config.search_db())?;
    let count: i64 = db.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?;
    let sqlite = start.elapsed();
    let start = Instant::now();
    let bytes = std::fs::read(config.parse_cache())?;
    let cache_read = start.elapsed();
    let start = Instant::now();
    let _: serde_json::Value = serde_json::from_slice(&bytes)?;
    let cache_parse = start.elapsed();
    let mut loads = Vec::new();
    let mut ranks = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let sessions = history::load_sessions(&config)?;
        loads.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        let _ = history::search(sessions, "signalbox", 10);
        ranks.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    loads.sort_by(f64::total_cmp);
    ranks.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"sessions":count,"cacheBytes":bytes.len(),
        "sqliteOpenAndCountMs":sqlite.as_secs_f64()*1000.0,
        "cacheReadMs":cache_read.as_secs_f64()*1000.0,"cacheDecodeMs":cache_parse.as_secs_f64()*1000.0,
        "historyLoadMedianMs":loads[10],"fuzzyRankMedianMs":ranks[10]}))?
    );
    Ok(())
}
