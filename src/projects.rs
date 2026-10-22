//! Narrow project patches through Orca's verified repo.update RPC; no Orca DB writes.
use crate::{config::Config, rpc::RpcClient};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAX_ICON_BYTES: usize = 256 * 1024;

pub fn show(config: &Config, selector: &str) -> Result<Value> {
    let value = RpcClient::new(config).call("repo.show", json!({"repo":selector}))?;
    value
        .get("repo")
        .cloned()
        .context("Orca returned no project repository")
}

/// Preserve every field not explicitly supplied. Validate before calling a schema
/// that silently drops some unsupported values, then read back the persisted patch.
pub fn set(config: &Config, selector: &str, updates: Value) -> Result<Value> {
    validate_updates(&updates)?;
    let before = show(config, selector)?;
    let id = before["id"]
        .as_str()
        .context("Project has no repository ID")?;
    let stable = format!("id:{id}");
    RpcClient::new(config).call("repo.update", json!({"repo":stable,"updates":updates}))?;
    let after = show(config, &stable)?;
    for (key, value) in updates.as_object().unwrap() {
        ensure!(
            after.get(key).unwrap_or(&Value::Null) == value,
            "Orca did not persist project field {key}; inspect projects settings show"
        );
    }
    Ok(after)
}

pub fn validate_updates(updates: &Value) -> Result<()> {
    let fields = updates
        .as_object()
        .context("Project settings must be an object")?;
    ensure!(!fields.is_empty(), "Supply at least one project setting");
    for (key, value) in fields {
        match key.as_str() {
            "displayName" | "worktreeBaseRef" | "worktreeBasePath" => {
                let text = value.as_str().context("Project setting must be text")?;
                ensure!(
                    !text.chars().any(char::is_control) && text.len() <= 4096,
                    "Invalid project setting text"
                );
            }
            "badgeColor" => {
                let s = value.as_str().context("Badge color must be #rrggbb")?;
                ensure!(
                    s.len() == 7
                        && s.starts_with('#')
                        && s[1..].bytes().all(|b| b.is_ascii_hexdigit()),
                    "Badge color must be #rrggbb"
                );
                ensure!(s == s.to_lowercase(), "Badge color must use lowercase hex");
            }
            "externalWorktreeVisibility" | "agentWorktreeVisibility" => {
                ensure!(
                    value.is_null() || matches!(value.as_str(), Some("show" | "hide")),
                    "Visibility must be show, hide, or null (inherit)"
                );
            }
            "issueSourcePreference" => ensure!(
                matches!(value.as_str(), Some("auto" | "upstream" | "origin")),
                "Invalid issue source"
            ),
            "forkSyncMode" => ensure!(
                matches!(value.as_str(), Some("ask" | "safe-auto" | "off")),
                "Invalid fork sync mode"
            ),
            "repoIcon" => validate_icon(value)?,
            _ => anyhow::bail!(
                "Unsupported project setting {key}; use raw RPC for other version-specific fields"
            ),
        }
    }
    Ok(())
}

fn validate_icon(value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    ensure!(
        value["type"] == "image" && value["source"] == "upload",
        "Use a PNG upload icon"
    );
    let encoded = value["src"]
        .as_str()
        .and_then(|s| s.strip_prefix("data:image/png;base64,"))
        .context("Expected a PNG data URI")?;
    ensure!(
        encoded.len() <= MAX_ICON_BYTES.div_ceil(3) * 4,
        "PNG icon exceeds 256 KiB"
    );
    validate_png(&STANDARD.decode(encoded)?)?;
    Ok(())
}

fn validate_png(bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_ICON_BYTES, "PNG icon exceeds 256 KiB");
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 32 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().context("Invalid PNG icon")?;
    let info = reader.info();
    ensure!(
        info.width > 0 && info.width == info.height && info.width <= 1024,
        "PNG icon must be square, between 1 and 1024 pixels (256 recommended)"
    );
    ensure!(info.animation_control.is_none(), "Use a static PNG icon");
    let size = reader
        .output_buffer_size()
        .context("PNG decoded size exceeds limits")?;
    ensure!(size <= 32 * 1024 * 1024, "PNG decoded size exceeds limits");
    reader
        .next_frame(&mut vec![0; size])
        .context("Invalid PNG image data")?;
    reader.finish().context("Invalid PNG image ending")?;
    Ok(())
}

fn read_icon(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_ICON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    validate_png(&bytes)?;
    Ok(bytes)
}

/// Import an existing PNG into its owning repository, refusing to replace artwork.
/// Orca receives only this icon field, and local runtime metadata never enters Git.
pub fn import_icon(config: &Config, selector: &str, path: &Path) -> Result<Value> {
    let bytes = read_icon(path)?;
    let repo = show(config, selector)?;
    let root = Path::new(
        repo["path"]
            .as_str()
            .context("Project has no local repository path")?,
    )
    .canonicalize()?;
    ensure!(
        root.join(".git").exists(),
        "Icon import requires a local Git repository"
    );
    let assets = root.join("Assets/Orca/ProjectIcon");
    // Validate ancestors before creating directories so a symlink cannot redirect a write.
    for ancestor in [
        root.join("Assets"),
        root.join("Assets/Orca"),
        assets.clone(),
    ] {
        if fs::symlink_metadata(&ancestor).is_ok() {
            ensure!(
                ancestor.canonicalize()?.starts_with(&root),
                "Artwork directory escapes its repository"
            );
        }
    }
    fs::create_dir_all(&assets)?;
    for name in ["master.png", "icon.png"] {
        let output = assets.join(name);
        if let Ok(existing) = fs::symlink_metadata(&output) {
            ensure!(
                existing.is_file() && fs::read(&output)? == bytes,
                "Existing artwork differs at {}; preserve or move it explicitly first",
                output.display()
            );
        }
    }
    for name in ["master.png", "icon.png"] {
        write_new_or_equal(&assets.join(name), &bytes)?;
    }
    let readme = assets.join("README.md");
    if !readme.exists() {
        write_new_or_equal(&readme, b"# Orca project icon\n\n![Project icon](icon.png)\n\nImported from an existing PNG with Orca Bridge. master.png preserves the imported source; icon.png is the upload copy.\n\nApply from the repository root:\n\n```sh\norca-bridge projects icon apply path:\"$PWD\" --file Assets/Orca/ProjectIcon/icon.png\n```\n")?;
    }
    Ok(json!({"imported":true,"directory":assets,"icon":assets.join("icon.png"),"applied":false}))
}

fn write_new_or_equal(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(path)?.is_file() && fs::read(path)? == bytes,
                "Refusing to replace existing artwork {}",
                path.display()
            );
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

pub fn apply_icon(config: &Config, selector: &str, path: &Path) -> Result<Value> {
    let bytes = read_icon(path)?;
    let label = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Project icon");
    let icon = json!({"type":"image","source":"upload","src":format!("data:image/png;base64,{}",STANDARD.encode(&bytes)),"label":label.chars().take(80).collect::<String>()});
    let repo = set(config, selector, json!({"repoIcon":icon}))?;
    Ok(
        json!({"applied":true,"repoId":repo["id"],"icon":{"type":"image","source":"upload","label":label,"bytes":bytes.len()}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = png::Encoder::new(&mut out, 2, 2).write_header().unwrap();
            writer.write_image_data(&[0, 1, 2, 3]).unwrap();
        }
        out
    }
    #[test]
    fn png_validation_decodes_and_rejects_bad_or_oversized_images() {
        let bytes = png_bytes();
        validate_png(&bytes).unwrap();
        assert!(validate_png(&bytes[..bytes.len() - 15]).is_err());
        assert!(validate_png(b"not a png").is_err());
        assert!(validate_png(&vec![0; MAX_ICON_BYTES + 1]).is_err());
    }
    #[test]
    fn only_explicit_supported_settings_are_accepted() {
        validate_updates(&json!({"displayName":"Bridge","externalWorktreeVisibility":null}))
            .unwrap();
        for value in [
            json!({}),
            json!({"unknown":true}),
            json!({"badgeColor":"invalid"}),
            json!({"displayName":null}),
            json!({"repoIcon":{"type":"image","src":"https://example.test"}}),
        ] {
            assert!(validate_updates(&value).is_err());
        }
    }
    #[test]
    fn artwork_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("icon.png");
        write_new_or_equal(&p, b"original").unwrap();
        write_new_or_equal(&p, b"original").unwrap();
        assert!(write_new_or_equal(&p, b"replacement").is_err());
        assert_eq!(fs::read(p).unwrap(), b"original");
    }
}
