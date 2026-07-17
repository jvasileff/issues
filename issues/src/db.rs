use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::model::{Issue, Status};

pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS issues (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'idea'
                CHECK (status IN ('idea','agreed','in-progress','done','abandoned')),
    body        TEXT NOT NULL DEFAULT '',
    parent_id   INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

const COLS: &str = "id, title, status, body, parent_id, created_at, updated_at";

/// RFC 3339 UTC with microsecond precision. Fixed width, so string
/// comparison agrees with chronological comparison.
fn fmt_ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

pub fn now_ts() -> String {
    fmt_ts(Utc::now())
}

/// Next `updated_at` value: strictly greater than `prev`, so it never
/// collides as the optimistic-lock token for `edit`.
pub fn next_ts(prev: &str) -> String {
    let now = fmt_ts(Utc::now());
    if now.as_str() > prev {
        return now;
    }
    match DateTime::parse_from_rfc3339(prev) {
        Ok(p) => fmt_ts(p.with_timezone(&Utc) + chrono::Duration::microseconds(1)),
        Err(_) => now,
    }
}

/// Walk up from CWD looking for a directory containing `.issues/`.
pub fn find_root() -> Result<PathBuf> {
    let mut dir = env::current_dir().context("cannot determine current directory")?;
    loop {
        if dir.join(".issues").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("not in an issues project; run `issues init`");
        }
    }
}

fn configure(conn: &Connection) -> Result<()> {
    let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    Ok(())
}

pub fn init(cwd: &Path) -> Result<()> {
    let dir = cwd.join(".issues");
    std::fs::create_dir_all(dir.join("drafts"))
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "*\n")?;
    }
    let conn = Connection::open(dir.join("issues.db"))?;
    configure(&conn)?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

pub fn open(root: &Path) -> Result<Connection> {
    let path = root.join(".issues").join("issues.db");
    if !path.is_file() {
        bail!("no database at {}; run `issues init`", path.display());
    }
    let conn =
        Connection::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    configure(&conn)?;
    let ver: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .context("database has no schema_version; not an issues database?")?;
    let ver_num: i64 = ver.parse().unwrap_or(i64::MAX);
    if ver_num > SCHEMA_VERSION {
        bail!(
            "database schema version {ver} is newer than this binary supports ({SCHEMA_VERSION}); upgrade `issues`"
        );
    }
    Ok(conn)
}

fn issue_from_row(row: &Row) -> rusqlite::Result<Issue> {
    let status: String = row.get(2)?;
    Ok(Issue {
        id: row.get(0)?,
        title: row.get(1)?,
        // The CHECK constraint guarantees a valid value.
        status: status.parse().unwrap_or(Status::Idea),
        body: row.get(3)?,
        parent_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue> {
    conn.query_row(
        &format!("SELECT {COLS} FROM issues WHERE id=?1"),
        [id],
        issue_from_row,
    )
    .optional()?
    .ok_or_else(|| anyhow!("issue #{id} not found"))
}

pub fn issue_exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM issues WHERE id=?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn all_issues(conn: &Connection) -> Result<Vec<Issue>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM issues"))?;
    let rows = stmt.query_map([], issue_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn add_issue(
    conn: &Connection,
    title: &str,
    status: Status,
    parent: Option<i64>,
    body: &str,
) -> Result<i64> {
    let ts = now_ts();
    conn.execute(
        "INSERT INTO issues(title, status, body, parent_id, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?5)",
        params![title, status.as_str(), body, parent, ts],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Apply `apply` to the issue inside a single immediate transaction,
/// bumping `updated_at` past its previous value.
pub fn modify_issue<F>(conn: &mut Connection, id: i64, apply: F) -> Result<Issue>
where
    F: FnOnce(&mut Issue) -> Result<()>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut issue = tx
        .query_row(
            &format!("SELECT {COLS} FROM issues WHERE id=?1"),
            [id],
            issue_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow!("issue #{id} not found"))?;
    let prev = issue.updated_at.clone();
    apply(&mut issue)?;
    issue.updated_at = next_ts(&prev);
    tx.execute(
        "UPDATE issues SET title=?1, status=?2, body=?3, parent_id=?4, updated_at=?5 WHERE id=?6",
        params![
            issue.title,
            issue.status.as_str(),
            issue.body,
            issue.parent_id,
            issue.updated_at,
            id
        ],
    )?;
    tx.commit()?;
    Ok(issue)
}

pub enum EditCommit {
    Committed,
    Stale,
}

/// Optimistic-locked write-back for the `edit` flow (§8.5): the UPDATE is
/// guarded by `WHERE updated_at = base_token`; zero affected rows means the
/// row changed underneath us and the caller must merge.
pub fn commit_edit(
    conn: &mut Connection,
    id: i64,
    title: &str,
    status: Status,
    parent: Option<i64>,
    body: &str,
    base_token: &str,
) -> Result<EditCommit> {
    // Hidden test hook: simulate a crash immediately before the commit
    // transaction, to prove the draft trio survives.
    if env::var("ISSUES_CRASH_BEFORE_COMMIT").as_deref() == Ok("1") {
        std::process::exit(3);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let cur: Option<String> = tx
        .query_row("SELECT updated_at FROM issues WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(cur) = cur else {
        bail!("issue #{id} not found")
    };
    if cur != base_token {
        return Ok(EditCommit::Stale);
    }
    let ts = next_ts(&cur);
    let n = tx.execute(
        "UPDATE issues SET title=?1, status=?2, body=?3, parent_id=?4, updated_at=?5
         WHERE id=?6 AND updated_at=?7",
        params![title, status.as_str(), body, parent, ts, id, base_token],
    )?;
    if n == 0 {
        return Ok(EditCommit::Stale);
    }
    tx.commit()?;
    Ok(EditCommit::Committed)
}
