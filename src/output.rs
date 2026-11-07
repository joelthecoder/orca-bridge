//! Compact projections preserve envelope fields that describe incomplete/unknown results.
use anyhow::{Result, ensure};
use serde_json::{Map, Value};
use std::sync::OnceLock;

static FIELDS: OnceLock<Vec<String>> = OnceLock::new();
const METADATA: &[&str] = &[
    "schemaVersion",
    "capturedAt",
    "collectedAtMs",
    "runtimeId",
    "scope",
    "processMemoryMetric",
    "selection",
    "terminalCount",
    "terminalsTruncated",
    "historyTruncated",
    "worktreesTruncated",
    "totalCount",
    "truncated",
    "inventoryTruncated",
    "inventoryTotalCount",
    "scanLimited",
    "measuredCount",
    "hostScope",
    "sourceTruncated",
    "limitTruncated",
    "incompleteFinalRecord",
    "bytesRead",
    "available",
    "error",
    "errors",
    "warnings",
    "bindingsVerified",
    "action",
    "dryRun",
    "allowed",
    "blockers",
    "effects",
    "checkedAt",
    "nativeTotalCount",
    "discoveredCount",
    "gitDiscoveredCount",
    "excludedExternalCount",
    "repositoryCount",
    "inspectedRepoCount",
    "unavailableRepoCount",
    "unavailableRepos",
];

pub fn configure(fields: Vec<String>) -> Result<()> {
    ensure!(
        fields.len() <= 100,
        "At most 100 output fields may be selected"
    );
    ensure!(
        fields.iter().all(|field| !field.is_empty()
            && field.len() <= 128
            && !field.chars().any(char::is_control)),
        "Output fields must be nonempty top-level names of at most 128 bytes"
    );
    FIELDS
        .set(fields)
        .map_err(|_| anyhow::anyhow!("Output fields were already configured"))
}

pub fn project_configured(value: &Value) -> Result<Value> {
    project(value, FIELDS.get().map(Vec::as_slice).unwrap_or(&[]))
}

pub fn project(value: &Value, fields: &[String]) -> Result<Value> {
    if fields.is_empty() {
        return Ok(value.clone());
    }
    if let Some(rows) = value.as_array() {
        return rows.iter().map(|row| project(row, fields)).collect();
    }
    let source = value.as_object().ok_or_else(|| {
        anyhow::anyhow!("Field selection needs JSON objects or arrays of objects")
    })?;
    let mut result = Map::new();
    for name in fields {
        result.insert(
            name.clone(),
            source.get(name).cloned().unwrap_or(Value::Null),
        );
    }
    for name in METADATA {
        if let Some(value) = source.get(*name) {
            result.insert((*name).into(), value.clone());
        }
    }
    Ok(Value::Object(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projection_keeps_requested_fields_and_completeness_metadata() {
        let source = json!({"schemaVersion":1,"terminals":[1],"app":{"memoryBytes":10},"terminalsTruncated":true,"terminalCount":7,"warnings":["partial"],"unknownField":"omit"});
        let result = project(&source, &["app".into(), "missing".into()]).unwrap();
        assert_eq!(
            result,
            json!({"schemaVersion":1,"app":{"memoryBytes":10},"missing":null,"terminalsTruncated":true,"terminalCount":7,"warnings":["partial"]})
        );
    }

    #[test]
    fn list_projection_is_per_row_and_does_not_invent_missing_values() {
        assert_eq!(
            project(
                &json!([{"id":"a","title":"A"},{"id":"b"}]),
                &["title".into()]
            )
            .unwrap(),
            json!([{"title":"A"},{"title":null}])
        );
        assert_eq!(project(&json!([]), &["id".into()]).unwrap(), json!([]));
        assert!(project(&json!("raw"), &["id".into()]).is_err());
    }
}
