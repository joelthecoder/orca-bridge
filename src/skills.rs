//! The app's bundled portable skill, installed only on explicit request.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub const MARKDOWN: &str = include_str!("../skills/orca-bridge/SKILL.md");

pub fn install(base: &Path) -> Result<PathBuf> {
    let root = base.join("skills/orca-bridge");
    let file = root.join("SKILL.md");
    for path in [&root, &file] {
        if let Ok(meta) = fs::symlink_metadata(path) {
            ensure!(
                !meta.file_type().is_symlink(),
                "Existing skill symlink preserved at {}; use `orca-bridge skill` to inspect the bundled version",
                path.display()
            );
        }
    }
    if file.exists() {
        ensure!(
            fs::read_to_string(&file)? == MARKDOWN,
            "Existing different skill preserved at {}; review `orca-bridge skill` before updating it",
            file.display()
        );
        return Ok(file);
    }
    if root.exists() {
        ensure!(
            fs::read_dir(&root)?.next().is_none(),
            "Existing skill directory preserved at {}",
            root.display()
        );
    }
    fs::create_dir_all(&root)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .context("Cannot create skill file")?;
    output.write_all(MARKDOWN.as_bytes())?;
    output.sync_all()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_is_idempotent_and_preserves_existing_skills() {
        let dir = tempfile::tempdir().unwrap();
        let file = install(dir.path()).unwrap();
        assert_eq!(install(dir.path()).unwrap(), file);
        fs::write(&file, "user skill").unwrap();
        assert!(install(dir.path()).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "user skill");
    }
    #[test]
    fn install_refuses_skill_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("own");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(dir.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(target, dir.path().join("skills/orca-bridge")).unwrap();
        assert!(install(dir.path()).is_err());
    }
}
