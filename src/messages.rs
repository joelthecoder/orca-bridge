//! A bounded, text-only projection of supported provider transcripts.
//!
//! This is not a complete conversation export: tools, thinking, non-text content,
//! and records outside the selected source tail are deliberately omitted.
use crate::model::Session;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
};

/// Read the most recent matching textual messages in chronological source order.
///
/// `max_bytes` bounds the source tail, not the normalized output size. One extra
/// byte may be read to determine whether the tail begins on a record boundary.
/// `sourceOffset` is the absolute byte offset of the original JSONL record.
pub fn read(
    session: &Session,
    limit: usize,
    role: Option<&str>,
    max_bytes: usize,
) -> Result<Value> {
    ensure!(limit > 0, "Message limit must be greater than zero");
    ensure!(
        max_bytes > 0,
        "Message byte limit must be greater than zero"
    );
    ensure!(
        role.is_none_or(|value| matches!(value, "user" | "assistant")),
        "Message role must be user or assistant"
    );
    ensure!(
        matches!(session.agent.as_str(), "codex" | "claude"),
        "Normalized messages are supported only for codex and claude"
    );
    ensure!(session.host == "local", "Messages require a local session");
    let path = session
        .file_path
        .as_deref()
        .context("This session has no local transcript file")?;
    ensure!(
        std::fs::metadata(path)
            .context("Cannot inspect local transcript")?
            .is_file(),
        "Transcript is not a regular local file"
    );
    let mut file = File::open(path).context("Cannot open local transcript")?;
    let metadata = file.metadata().context("Cannot inspect local transcript")?;
    ensure!(metadata.is_file(), "Transcript is not a regular local file");
    // Take a fixed-length snapshot even when the provider is appending records.
    let length = metadata.len();
    let start = length.saturating_sub(max_bytes as u64);
    let mut bytes_read = 0u64;
    let discard_leading = if start > 0 {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0u8; 1];
        file.read_exact(&mut previous)?;
        bytes_read += 1;
        previous[0] != b'\n'
    } else {
        false
    };
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(length - start));
    let mut offset = start;
    let mut line = Vec::new();
    if discard_leading {
        let discarded = reader.read_until(b'\n', &mut line)? as u64;
        offset += discarded;
        bytes_read += discarded;
        line.clear();
    }
    let mut messages = VecDeque::new();
    let mut limit_truncated = false;
    let mut incomplete_final_record = false;
    loop {
        line.clear();
        let count = reader.read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        let source_offset = offset;
        offset += count as u64;
        bytes_read += count as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let complete_line = line.last() == Some(&b'\n');
        let record: Value = match serde_json::from_slice(&line) {
            Ok(record) => record,
            Err(error) if !complete_line && error.is_eof() => {
                incomplete_final_record = true;
                break;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Invalid transcript JSON at byte {source_offset}"));
            }
        };
        if let Some(message) = project(&record, &session.agent, role, source_offset) {
            if messages.len() == limit {
                messages.pop_front();
                limit_truncated = true;
            }
            messages.push_back(message);
        }
    }
    Ok(json!({
        "id": session.id,
        "agent": session.agent,
        "messages": messages,
        "sourceTruncated": start > 0,
        "limitTruncated": limit_truncated,
        "incompleteFinalRecord": incomplete_final_record,
        "bytesRead": bytes_read,
    }))
}

fn project(record: &Value, agent: &str, role: Option<&str>, offset: u64) -> Option<Value> {
    let message = if agent == "codex" {
        if record["type"] != "response_item" || record["payload"]["type"] != "message" {
            return None;
        }
        &record["payload"]
    } else {
        if !matches!(record["type"].as_str(), Some("user" | "assistant")) {
            return None;
        }
        &record["message"]
    };
    let source_role = message["role"].as_str().or_else(|| {
        if agent == "claude" {
            record["type"].as_str()
        } else {
            None
        }
    })?;
    if !matches!(source_role, "user" | "assistant")
        || role.is_some_and(|selected| selected != source_role)
    {
        return None;
    }
    let content = &message["content"];
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        content
            .as_array()?
            .iter()
            .filter(|block| {
                if agent == "codex" {
                    matches!(
                        block["type"].as_str(),
                        Some("input_text" | "output_text" | "text")
                    )
                } else {
                    block["type"] == "text"
                }
            })
            .filter_map(|block| block["text"].as_str())
            .collect::<String>()
    };
    if text.is_empty() {
        return None;
    }
    Some(json!({
        "role": source_role,
        "text": text,
        "timestamp": record.get("timestamp"),
        "phase": message.get("phase").or_else(|| record.get("phase")),
        "sourceOffset": offset,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(agent: &str, contents: &[u8]) -> (tempfile::NamedTempFile, Session) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(contents).unwrap();
        let session = Session {
            id: format!("local:{agent}:fixture"),
            host: "local".into(),
            agent: agent.into(),
            session_id: "fixture".into(),
            title: "Fixture".into(),
            original_title: None,
            cwd: None,
            branch: None,
            model: None,
            created_at: None,
            updated_at: None,
            message_count: 0,
            file_path: Some(file.path().to_str().unwrap().into()),
            codex_home: None,
            total_tokens: None,
            usage: None,
            score: None,
        };
        (file, session)
    }

    fn codex(role: &str, text: &str) -> String {
        json!({"type":"response_item","payload":{"type":"message","role":role,
            "content":[{"type":"output_text","text":text}]}})
        .to_string()
            + "\n"
    }

    #[test]
    fn codex_uses_canonical_messages_and_preserves_source_metadata() {
        let contents = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"duplicate\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"commentary\",\"content\":[{\"type\":\"output_text\",\"text\":\"First\"},{\"type\":\"thinking\",\"text\":\"private\"},{\"type\":\"text\",\"text\":\" second\"}]}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"tool\"}}\n"
        );
        let (_file, session) = fixture("codex", contents.as_bytes());
        let result = read(&session, 10, None, 4096).unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        let message = &result["messages"][0];
        assert_eq!(message["text"], "First second");
        assert_eq!(message["phase"], "commentary");
        assert_eq!(message["timestamp"], "2026-01-01T00:00:00Z");
        assert_eq!(
            message["sourceOffset"],
            contents.find('\n').unwrap() as u64 + 1
        );
        assert_eq!(result["bytesRead"], contents.len() as u64);
    }

    #[test]
    fn claude_projects_strings_and_text_blocks_but_not_tools_or_thinking() {
        let contents = concat!(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Question\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"private\"},{\"type\":\"text\",\"text\":\"Answer\"},{\"type\":\"tool_use\",\"text\":\"omit\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"tool output\"}]}}\n",
            "{\"type\":\"system\",\"message\":{\"role\":\"user\",\"content\":\"omit\"}}\n"
        );
        let (_file, session) = fixture("claude", contents.as_bytes());
        let result = read(&session, 10, None, 4096).unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 2);
        assert_eq!(result["messages"][0]["text"], "Question");
        assert_eq!(result["messages"][1]["text"], "Answer");
        assert!(result["messages"][1]["phase"].is_null());
    }

    #[test]
    fn role_filter_applies_before_recent_limit_and_reports_dropped_matches() {
        let contents =
            codex("assistant", "Old") + &codex("user", "Question") + &codex("assistant", "New");
        let (_file, session) = fixture("codex", contents.as_bytes());
        let result = read(&session, 1, Some("assistant"), 4096).unwrap();
        assert_eq!(result["messages"][0]["text"], "New");
        assert_eq!(result["limitTruncated"], true);
        assert_eq!(result["sourceTruncated"], false);
        let result = read(&session, 1, Some("user"), 4096).unwrap();
        assert_eq!(result["messages"][0]["text"], "Question");
        assert_eq!(result["limitTruncated"], false);
    }

    #[test]
    fn tail_retains_a_complete_boundary_and_absolute_offsets() {
        let first = codex("user", "Old");
        let last = codex("assistant", "Recent");
        let contents = first.clone() + &last;
        let (_file, session) = fixture("codex", contents.as_bytes());
        let result = read(&session, 10, None, last.len()).unwrap();
        assert_eq!(result["messages"][0]["text"], "Recent");
        assert_eq!(result["messages"][0]["sourceOffset"], first.len() as u64);
        assert_eq!(result["sourceTruncated"], true);
        assert_eq!(result["bytesRead"], last.len() as u64 + 1);
    }

    #[test]
    fn tail_drops_partial_leading_record_without_parsing_it() {
        let first = codex("user", "Old 🐋");
        let last = codex("assistant", "Recent");
        let contents = first.clone() + &last;
        let (_file, session) = fixture("codex", contents.as_bytes());
        let result = read(&session, 10, None, last.len() + 5).unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        assert_eq!(result["messages"][0]["sourceOffset"], first.len() as u64);
        assert_eq!(result["sourceTruncated"], true);
    }

    #[test]
    fn incomplete_final_record_is_skipped_but_complete_non_newline_record_is_kept() {
        let complete = codex("assistant", "Ready");
        let contents = complete.clone() + "{\"type\":\"response_item\",\"payload\":";
        let (_file, session) = fixture("codex", contents.as_bytes());
        let result = read(&session, 10, None, 4096).unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        assert_eq!(result["incompleteFinalRecord"], true);
        let (_file, session) = fixture("codex", complete.trim_end().as_bytes());
        let result = read(&session, 10, None, 4096).unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        assert_eq!(result["incompleteFinalRecord"], false);
    }

    #[test]
    fn invalid_complete_json_has_a_location_without_echoing_contents() {
        let (_file, session) = fixture("codex", b"{private-invalid-record}\n");
        let error = read(&session, 10, None, 4096).unwrap_err();
        let description = format!("{error:#}");
        assert!(description.contains("byte 0"));
        assert!(!description.contains("private-invalid-record"));
    }

    #[test]
    fn validates_provider_role_and_positive_bounds_and_supports_empty_files() {
        let (_file, session) = fixture("codex", b"");
        assert!(read(&session, 0, None, 4096).is_err());
        assert!(read(&session, 10, None, 0).is_err());
        assert!(read(&session, 10, Some("tool"), 4096).is_err());
        let result = read(&session, 10, None, 4096).unwrap();
        assert_eq!(result["messages"], json!([]));
        assert_eq!(result["bytesRead"], 0);
        let mut unsupported = session;
        unsupported.agent = "cursor".into();
        assert!(read(&unsupported, 10, None, 4096).is_err());
    }
}
