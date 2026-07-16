use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write as _};
use std::path::Path;
use std::process::{self, Command};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;

use crate::checkout::{self, ParseError};
use crate::drafts::{self, DraftPaths};
use crate::model::Status;
use crate::{db, merge};

/// True when the interactive commands may run: real TTYs on stdin and
/// stdout, or the hidden ISSUES_ASSUME_TTY=1 test hook.
pub fn tty_ok() -> bool {
    env::var("ISSUES_ASSUME_TTY").as_deref() == Ok("1")
        || (io::stdin().is_terminal() && io::stdout().is_terminal())
}

pub fn launch_editor(path: &Path) -> Result<()> {
    let editor = env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| env::var("EDITOR").ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    // `sh -c` so values like `code --wait` work.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .with_context(|| format!("failed to launch editor '{editor}'"))?;
    if !status.success() {
        bail!("editor '{editor}' exited unsuccessfully; any draft was kept");
    }
    Ok(())
}

pub fn edit(conn: &mut Connection, root: &Path, id: i64) -> Result<()> {
    let paths = DraftPaths::new(root, id);
    if paths.draft.exists() {
        resume(conn, root, &paths)
    } else {
        fresh_checkout(conn, root, &paths)
    }
}

pub fn resume_draft(conn: &mut Connection, root: &Path, id: i64) -> Result<()> {
    let paths = DraftPaths::new(root, id);
    if !paths.draft.exists() {
        bail!("no draft for #{id} (see 'issues drafts')");
    }
    resume(conn, root, &paths)
}

fn fresh_checkout(conn: &mut Connection, root: &Path, paths: &DraftPaths) -> Result<()> {
    let issue = db::get_issue(conn, paths.id)?;
    let text = checkout::render(&issue, true);
    paths.write_all(&text, &text, &issue.updated_at)?;
    launch_editor(&paths.draft)?;
    run_loop(
        conn,
        root,
        paths,
        LoopCtx {
            prev_text: text.clone(),
            base_text: text,
            base_token: issue.updated_at,
        },
    )
}

struct LoopCtx {
    /// Checkout-format text the draft's edits are relative to.
    base_text: String,
    /// `updated_at` of the row `base_text` was serialized from.
    base_token: String,
    /// Draft content as of the last editor launch (comment blocks stripped).
    /// "Saved with no changes" is judged against this.
    prev_text: String,
}

/// §8.3–§8.7: parse / retry / lock-check / merge / commit, until the edit
/// lands, the user aborts, or there was nothing to save.
fn run_loop(conn: &mut Connection, root: &Path, paths: &DraftPaths, mut ctx: LoopCtx) -> Result<()> {
    loop {
        let raw = fs::read_to_string(&paths.draft)
            .with_context(|| format!("cannot read draft {}", paths.display()))?;
        let stripped = strip_leading_comment_block(&raw);
        if stripped != raw {
            fs::write(&paths.draft, &stripped)?;
        }

        if stripped == ctx.prev_text {
            if ctx.prev_text == ctx.base_text {
                // Nothing differs from the base version: nothing to save.
                paths.delete()?;
                println!("no changes");
                return Ok(());
            }
            // Editor relaunch (parse error / conflict) closed without
            // further changes: abort, keeping the draft.
            eprintln!("aborted; draft kept at {} (see 'issues drafts')", paths.display());
            process::exit(1);
        }

        let parsed = match checkout::parse(&stripped) {
            Ok(p) => p,
            Err(e) => {
                relaunch_with_block(paths, &stripped, &error_block(&e))?;
                ctx.prev_text = stripped;
                continue;
            }
        };

        let base = checkout::parse(&ctx.base_text)
            .map_err(|e| anyhow!("internal error: stored base is unparsable ({e})"))?;
        let title = parsed.title.or(base.title).unwrap_or_default();
        let status = parsed.status.or(base.status).unwrap_or(Status::Idea);
        let parent = parsed.parent;

        if let Some(p) = parent {
            let problem = if p == paths.id {
                Some(format!("issue #{p} cannot be its own parent"))
            } else if !db::issue_exists(conn, p)? {
                Some(format!("parent issue #{p} does not exist"))
            } else {
                None
            };
            if let Some(msg) = problem {
                let block = format!(
                    "# ERROR: {msg}.\n# Fix and save, or close without further changes to abort.\n"
                );
                relaunch_with_block(paths, &stripped, &block)?;
                ctx.prev_text = stripped;
                continue;
            }
        }

        match db::commit_edit(conn, paths.id, &title, status, parent, &parsed.body, &ctx.base_token)? {
            db::EditCommit::Committed => {
                // Only now — after the transaction committed — may the
                // draft trio go away (§3).
                paths.delete()?;
                println!("#{} saved (status: {})", paths.id, status);
                return Ok(());
            }
            db::EditCommit::Stale => {
                let fresh = db::get_issue(conn, paths.id)?;
                let theirs_text = checkout::render(&fresh, true);
                match merge::merge_file(&paths.dir, paths.id, &ctx.base_text, &stripped, &theirs_text)? {
                    merge::MergeOutcome::Clean(merged) => {
                        fs::write(&paths.draft, &merged)?;
                        paths.write_base(&theirs_text, &fresh.updated_at)?;
                        ctx.prev_text = theirs_text.clone();
                        ctx.base_text = theirs_text;
                        ctx.base_token = fresh.updated_at;
                        println!("merged concurrent changes automatically");
                        // Loop re-reads the merged draft and commits it.
                    }
                    merge::MergeOutcome::Conflict(conflicted) => {
                        let header = "\
# CONFLICT: this issue was modified while you were editing (likely by Claude).
# Resolve the <<<<<<< / >>>>>>> sections, save, and close.
# Close without changes to abort (draft will be kept).
";
                        fs::write(&paths.draft, format!("{header}{conflicted}"))?;
                        paths.write_base(&theirs_text, &fresh.updated_at)?;
                        ctx.base_text = theirs_text;
                        ctx.base_token = fresh.updated_at;
                        ctx.prev_text = conflicted;
                        launch_editor(&paths.draft)?;
                    }
                    merge::MergeOutcome::NoGit => {
                        eprintln!(
                            "issue #{} was modified while you were editing, and `git` (required for automatic merge) is not on PATH.",
                            paths.id
                        );
                        let _ = drafts::print_diff(conn, root, paths.id);
                        eprintln!(
                            "draft kept at {}; reconcile manually with the scriptable commands, or install git and rerun 'issues edit {}'",
                            paths.display(),
                            paths.id
                        );
                        process::exit(1);
                    }
                }
            }
        }
    }
}

/// §8.6: a draft already exists for this issue.
fn resume(conn: &mut Connection, root: &Path, paths: &DraftPaths) -> Result<()> {
    println!(
        "an unsaved draft for #{} exists (from {} ago).",
        paths.id,
        drafts::file_age(&paths.draft)
    );
    loop {
        print!("[r]esume draft / [d]iff / [x] discard and start fresh / [q]uit? ");
        io::stdout().flush()?;
        let Some(answer) = read_line()? else { return Ok(()) };
        match answer.trim().to_ascii_lowercase().as_str() {
            "r" => {
                let (base_text, base_token) = match paths.read_base() {
                    Some(pair) => pair,
                    None => {
                        // Partial crash during checkout: fall back to the
                        // current row as base.
                        eprintln!(
                            "warning: stored base for the draft is missing; using the current issue as base (merge fidelity reduced)"
                        );
                        let cur = db::get_issue(conn, paths.id)?;
                        let text = checkout::render(&cur, true);
                        paths.write_base(&text, &cur.updated_at)?;
                        (text, cur.updated_at)
                    }
                };
                launch_editor(&paths.draft)?;
                return run_loop(
                    conn,
                    root,
                    paths,
                    LoopCtx { prev_text: base_text.clone(), base_text, base_token },
                );
            }
            "d" => {
                if let Err(e) = drafts::print_diff(conn, root, paths.id) {
                    eprintln!("error: {e}");
                }
            }
            "x" => {
                print!("discard draft for #{} and start fresh? [y/N] ", paths.id);
                io::stdout().flush()?;
                let Some(confirm) = read_line()? else { return Ok(()) };
                if matches!(confirm.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    paths.delete()?;
                    return fresh_checkout(conn, root, paths);
                }
            }
            "q" | "" => return Ok(()),
            _ => {}
        }
    }
}

fn read_line() -> Result<Option<String>> {
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None); // EOF
    }
    Ok(Some(line))
}

fn relaunch_with_block(paths: &DraftPaths, content: &str, block: &str) -> Result<()> {
    fs::write(&paths.draft, format!("{block}{content}"))?;
    launch_editor(&paths.draft)
}

fn error_block(e: &ParseError) -> String {
    let mut s = format!("# ERROR: {} on line {}.\n", e.msg, e.line);
    if let Some(h) = &e.hint {
        s.push_str(&format!("# {h}\n"));
    }
    s.push_str("# Fix and save, or close without further changes to abort.\n");
    s
}

/// Remove the contiguous `#`-comment block we prepend at the top of the
/// draft (ERROR / CONFLICT headers). Legitimate checkouts start with `---`.
fn strip_leading_comment_block(text: &str) -> String {
    let mut rest = text;
    while rest.starts_with('#') {
        match rest.find('\n') {
            Some(i) => rest = &rest[i + 1..],
            None => rest = "",
        }
    }
    rest.to_string()
}
