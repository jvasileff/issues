use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::{checkout, db, output};

/// The draft trio for one issue: the user's draft, the base version the
/// draft's edits are relative to, and the base's optimistic-lock token.
pub struct DraftPaths {
    pub id: i64,
    pub dir: PathBuf,
    pub draft: PathBuf,
    pub base: PathBuf,
    pub meta: PathBuf,
}

impl DraftPaths {
    pub fn new(root: &Path, id: i64) -> Self {
        let dir = root.join(".issues").join("drafts");
        DraftPaths {
            id,
            draft: dir.join(format!("{id}.md")),
            base: dir.join(format!("{id}.base.md")),
            meta: dir.join(format!("{id}.meta")),
            dir,
        }
    }

    /// Repo-relative display path for messages.
    pub fn display(&self) -> String {
        format!(".issues/drafts/{}.md", self.id)
    }

    pub fn write_all(&self, draft: &str, base: &str, token: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(&self.draft, draft)?;
        self.write_base(base, token)
    }

    pub fn write_base(&self, base: &str, token: &str) -> Result<()> {
        fs::write(&self.base, base)?;
        fs::write(&self.meta, format!("base_updated_at={token}\n"))?;
        Ok(())
    }

    pub fn read_base(&self) -> Option<(String, String)> {
        let base = fs::read_to_string(&self.base).ok()?;
        let meta = fs::read_to_string(&self.meta).ok()?;
        let token = meta.lines().find_map(|l| l.strip_prefix("base_updated_at="))?.trim().to_string();
        Some((base, token))
    }

    /// Delete the trio. Only call after the corresponding db transaction has
    /// committed (or the user explicitly discarded the draft).
    pub fn delete(&self) -> Result<()> {
        for p in [&self.draft, &self.base, &self.meta] {
            match fs::remove_file(p) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("cannot remove {}", p.display())),
            }
        }
        Ok(())
    }
}

pub struct DraftInfo {
    pub id: i64,
    pub age: String,
}

pub fn file_age(p: &Path) -> String {
    match fs::metadata(p).and_then(|m| m.modified()) {
        Ok(t) => output::relative_secs(t.elapsed().unwrap_or_default().as_secs(), "<1m"),
        Err(_) => "?".to_string(),
    }
}

pub fn list_drafts(root: &Path) -> Vec<DraftInfo> {
    let dir = root.join(".issues").join("drafts");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Only "<id>.md" counts; skips <id>.base.md, <id>.meta, *.tmp.
            if let Some(stem) = name.strip_suffix(".md")
                && !stem.contains('.')
                && let Ok(id) = stem.parse::<i64>()
            {
                out.push(DraftInfo { id, age: file_age(&e.path()) });
            }
        }
    }
    out.sort_by_key(|d| d.id);
    out
}

/// Unified diff between the current db row and the draft, via `diff -u`.
pub fn print_diff(conn: &Connection, root: &Path, id: i64) -> Result<()> {
    let paths = DraftPaths::new(root, id);
    if !paths.draft.exists() {
        bail!("no draft for #{id}");
    }
    let issue = db::get_issue(conn, id)?;
    let current = checkout::render(&issue, true);
    let tmp = paths.dir.join(format!("{id}.current.tmp"));
    fs::write(&tmp, &current)?;
    let out = Command::new("diff")
        .arg("-u")
        .args(["--label", &format!("#{id} (current)")])
        .args(["--label", &format!("#{id} (draft)")])
        .arg(&tmp)
        .arg(&paths.draft)
        .output();
    let _ = fs::remove_file(&tmp);
    let out = match out {
        Err(e) if e.kind() == ErrorKind::NotFound => bail!("`diff` not found on PATH"),
        other => other.context("failed to run diff")?,
    };
    match out.status.code() {
        Some(0) => println!("draft is identical to the current issue"),
        Some(1) => std::io::stdout().write_all(&out.stdout)?,
        _ => bail!("diff failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
    }
    Ok(())
}
