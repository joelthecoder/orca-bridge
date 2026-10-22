use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub orca_cli: PathBuf,
    pub profile_db_override: Option<PathBuf>,
}

impl Config {
    pub fn discover(data_dir: Option<PathBuf>, state_dir: Option<PathBuf>) -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let home = PathBuf::from(home);
        Ok(Self {
            data_dir: data_dir
                .or_else(|| std::env::var_os("ORCA_USER_DATA_PATH").map(PathBuf::from))
                .unwrap_or_else(|| home.join("Library/Application Support/orca")),
            state_dir: state_dir
                .or_else(|| std::env::var_os("ORCA_BRIDGE_STATE_DIR").map(PathBuf::from))
                .unwrap_or_else(|| home.join("Library/Application Support/orca-bridge")),
            orca_cli: std::env::var_os("ORCA_BRIDGE_ORCA_CLI")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from("/Applications/Orca.app/Contents/Resources/bin/orca")
                }),
            profile_db_override: None,
        })
    }

    pub fn search_db(&self) -> PathBuf {
        self.data_dir.join("ai-vault/session-search.sqlite")
    }
    pub fn parse_cache(&self) -> PathBuf {
        self.data_dir.join("ai-vault/session-parse-cache.json")
    }
    pub fn runtime_metadata(&self) -> PathBuf {
        self.data_dir.join("orca-runtime.json")
    }
    pub fn profile_db(&self) -> Result<PathBuf> {
        if let Some(path) = &self.profile_db_override {
            return Ok(path.clone());
        }
        let index_path = self.data_dir.join("orca-profile-index.json");
        if index_path.is_file() {
            let index: serde_json::Value = serde_json::from_slice(&std::fs::read(&index_path)?)?;
            if let Some(id) = index.get("activeProfileId").and_then(|v| v.as_str()) {
                anyhow::ensure!(
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
                    "Invalid active profile ID"
                );
                let path = self
                    .data_dir
                    .join("profiles")
                    .join(id)
                    .join("profile-state.db");
                anyhow::ensure!(
                    path.is_file(),
                    "Active profile database is missing: {}",
                    path.display()
                );
                return Ok(path);
            }
            anyhow::bail!("Profile index has no activeProfileId");
        }
        let profiles = self.data_dir.join("profiles");
        let mut candidates = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&profiles) {
            for entry in entries.flatten() {
                let path = entry.path().join("profile-state.db");
                if path.is_file() {
                    candidates.push(path);
                }
            }
        }
        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            0 => anyhow::bail!("No profile-state.db under {}", profiles.display()),
            _ => anyhow::bail!(
                "Multiple profiles found; select the active profile explicitly with --profile-db"
            ),
        }
    }
}

pub fn readonly_db(path: &Path) -> Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Cannot open {} read-only", path.display()))?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    db.pragma_update(None, "query_only", true)?;
    Ok(db)
}
