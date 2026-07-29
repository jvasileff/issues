use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::model::{Issue, Status};

/// v1: original schema (an 'issues' table; status vocabulary in a CHECK
/// constraint on it).
/// v2: table names singular ('issue'); vocabulary moved to the 'status'
/// lookup table; 'doc' added.
pub const SCHEMA_VERSION: i64 = 2;

/// The status table holds the fixed status vocabulary that issue.status
/// references. Its rows are part of the schema, not data: they change
/// only alongside an official SCHEMA_VERSION bump, staying in lockstep
/// with the Status enum, which is the source of truth.
const STATUS_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS status (name TEXT PRIMARY KEY);";

fn status_seed_sql() -> String {
    let rows: Vec<String> = Status::ALL
        .map(|s| format!("('{}')", s.as_str()))
        .into_iter()
        .collect();
    format!(
        "INSERT OR IGNORE INTO status(name) VALUES {};",
        rows.join(",")
    )
}

/// Column definitions for the issue table, shared by initial creation and
/// the migration rebuild. The status foreign key is enforced only on
/// connections with PRAGMA foreign_keys=ON, which configure() always sets.
const ISSUE_TABLE_BODY: &str = "(
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'idea' REFERENCES status(name),
    body        TEXT NOT NULL DEFAULT '',
    parent_id   INTEGER REFERENCES issue(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
)";

fn schema_sql() -> String {
    format!(
        "{STATUS_TABLE_SQL}
         {seed}
         CREATE TABLE IF NOT EXISTS issue {ISSUE_TABLE_BODY};

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
        seed = status_seed_sql()
    )
}

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
    let db_path = dir.join("issues.db");
    let existed = db_path.is_file();
    let conn = Connection::open(&db_path)?;
    configure(&conn)?;
    // Re-running init must not layer current-version tables onto an older
    // database; that would leave a hybrid the migration cannot rebuild. A
    // pre-existing file without a version (empty or partial) is fair game.
    if existed && let Ok(ver) = schema_version(&conn) {
        require_current(ver)?;
    }
    conn.execute_batch(&schema_sql())?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// Open the database with no schema-version gate: for check and upgrade,
/// which handle version mismatches themselves. Every other caller goes
/// through open().
pub fn open_raw(root: &Path) -> Result<Connection> {
    let path = root.join(".issues").join("issues.db");
    if !path.is_file() {
        bail!("no database at {}; run `issues init`", path.display());
    }
    let conn =
        Connection::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    configure(&conn)?;
    Ok(conn)
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    let ver: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .context("database has no schema_version; not an issues database?")?;
    ver.parse()
        .map_err(|_| anyhow!("schema_version '{ver}' is not a number"))
}

/// Fail unless the database is exactly this binary's schema version.
/// Migration is never implicit: an older database is only ever changed by
/// the explicit `issues upgrade` command.
pub fn require_current(ver: i64) -> Result<()> {
    if ver > SCHEMA_VERSION {
        bail!(
            "database schema version {ver} is newer than this binary supports ({SCHEMA_VERSION}); upgrade `issues`"
        );
    }
    if ver < SCHEMA_VERSION {
        bail!(
            "database schema version {ver} is older than this binary ({SCHEMA_VERSION}); run `issues upgrade`"
        );
    }
    Ok(())
}

pub fn open(root: &Path) -> Result<Connection> {
    let conn = open_raw(root)?;
    require_current(schema_version(&conn)?)?;
    Ok(conn)
}

/// Upgrade an older database in place. v1 kept the status vocabulary in a
/// CHECK constraint on the 'issues' table; v2 moves it to the 'status'
/// lookup table, so a future vocabulary change is a seed INSERT plus a
/// version bump. Leaving v1 takes one table rebuild (SQLite cannot drop a
/// CHECK constraint); v2 also renames the table to the singular 'issue',
/// so the rebuilt table is created directly under its final name, the rows
/// copied over, and the old table dropped. Copying explicit ids keeps
/// sqlite_sequence at the current max id.
///
/// Called only by the upgrade command, never on ordinary open: the
/// caller is responsible for the surrounding ritual (pre-flight checks,
/// backup, post-upgrade check).
pub fn migrate(conn: &mut Connection) -> Result<()> {
    // Foreign-key enforcement must be off during the copy: rows arrive in
    // id order, and a child's parent_id may point at a higher id that has
    // not been copied yet. The pragma is a no-op inside a transaction, so
    // toggle it around one.
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let res = (|| -> Result<()> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-read the version under the write lock: a concurrent process
        // may have migrated between our version check and here.
        let ver: String = tx.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )?;
        if ver.parse::<i64>().unwrap_or(i64::MAX) < SCHEMA_VERSION {
            tx.execute_batch(&format!(
                "{STATUS_TABLE_SQL}
                 {seed}
                 CREATE TABLE issue {ISSUE_TABLE_BODY};
                 INSERT INTO issue SELECT {COLS} FROM issues;
                 DROP TABLE issues;",
                seed = status_seed_sql()
            ))?;
            tx.execute(
                "UPDATE meta SET value=?1 WHERE key='schema_version'",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    res
}

fn issue_from_row(row: &Row) -> rusqlite::Result<Issue> {
    let status: String = row.get(2)?;
    Ok(Issue {
        id: row.get(0)?,
        title: row.get(1)?,
        // The status foreign key keeps stored values in the vocabulary.
        status: status.parse().unwrap_or(Status::Idea),
        body: row.get(3)?,
        parent_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue> {
    conn.query_row(
        &format!("SELECT {COLS} FROM issue WHERE id=?1"),
        [id],
        issue_from_row,
    )
    .optional()?
    .ok_or_else(|| anyhow!("issue #{id} not found"))
}

pub fn issue_exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM issue WHERE id=?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn all_issues(conn: &Connection) -> Result<Vec<Issue>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM issue"))?;
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
        "INSERT INTO issue(title, status, body, parent_id, created_at, updated_at)
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
            &format!("SELECT {COLS} FROM issue WHERE id=?1"),
            [id],
            issue_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow!("issue #{id} not found"))?;
    let prev = issue.updated_at.clone();
    apply(&mut issue)?;
    issue.updated_at = next_ts(&prev);
    tx.execute(
        "UPDATE issue SET title=?1, status=?2, body=?3, parent_id=?4, updated_at=?5 WHERE id=?6",
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
        .query_row("SELECT updated_at FROM issue WHERE id=?1", [id], |r| {
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
        "UPDATE issue SET title=?1, status=?2, body=?3, parent_id=?4, updated_at=?5
         WHERE id=?6 AND updated_at=?7",
        params![title, status.as_str(), body, parent, ts, id, base_token],
    )?;
    if n == 0 {
        return Ok(EditCommit::Stale);
    }
    tx.commit()?;
    Ok(EditCommit::Committed)
}
