use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub enum MergeOutcome {
    Clean(String),
    Conflict(String),
    /// `git` is not on PATH; automatic merge unavailable.
    NoGit,
}

/// 3-way merge of checkout-format texts via `git merge-file -p`.
/// Temp copies live inside the drafts dir and are removed afterwards.
pub fn merge_file(
    dir: &Path,
    id: i64,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<MergeOutcome> {
    let b = dir.join(format!("{id}.base.tmp"));
    let o = dir.join(format!("{id}.ours.tmp"));
    let t = dir.join(format!("{id}.theirs.tmp"));
    fs::write(&o, ours)?;
    fs::write(&b, base)?;
    fs::write(&t, theirs)?;
    let out = Command::new("git")
        .args(["merge-file", "-p"])
        .args(["-L", "your edit", "-L", "base", "-L", "concurrent change"])
        .arg(&o)
        .arg(&b)
        .arg(&t)
        .output();
    for p in [&b, &o, &t] {
        let _ = fs::remove_file(p);
    }
    let out = match out {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(MergeOutcome::NoGit),
        other => other.context("failed to run git merge-file")?,
    };
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    match out.status.code() {
        Some(0) => Ok(MergeOutcome::Clean(text)),
        // git merge-file exits with the number of conflicts (< 128).
        Some(n) if n > 0 && n < 128 => Ok(MergeOutcome::Conflict(text)),
        _ => bail!(
            "git merge-file failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}
