use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Durable bridge ID: local:<percent-encoded-agent>:<percent-encoded-provider-ID>.
    pub id: String,
    /// Execution host identity; history currently supports local.
    pub host: String,
    /// Provider/agent ID, such as codex or claude.
    pub agent: String,
    /// Original provider conversation ID. Never use this as a terminal handle.
    pub session_id: String,
    /// Indexed provider title. A live tab's custom title is a separate field.
    pub title: String,
    /// Original indexed title when a bridge-owned display title overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    /// Indexed working directory, not proof of live session ownership.
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// Fresh partial-cache model; null means unknown.
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: i64,
    /// Local source transcript path; may be unavailable for database-backed providers.
    pub file_path: Option<String>,
    /// Account-specific Codex home path, never a credential value.
    pub codex_home: Option<String>,
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
}
