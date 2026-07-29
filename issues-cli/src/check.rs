//! Database self-audit shared by `issues check` and `issues upgrade`.
//! Each check prints one `ok:`/`FAIL:` line (failures add indented
//! detail) and the runners return whether everything passed.

use anyhow::Result;
use rusqlite::Connection;

use crate::checkout::validate_title;
use crate::model::Status;

fn report(name: &str, failures: &[String]) -> bool {
    if failures.is_empty() {
        println!("ok: {name}");
        true
    } else {
        println!("FAIL: {name}");
        for f in failures {
            println!("  {f}");
        }
        false
    }
}

fn integrity(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter(|m| m != "ok").collect())
}

fn foreign_keys(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (table, rowid, parent) = row?;
        let rowid = rowid.map_or_else(|| "?".to_string(), |n| n.to_string());
        out.push(format!(
            "{table} row {rowid}: dangling reference into {parent}"
        ));
    }
    Ok(out)
}

fn vocabulary(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM status ORDER BY name")?;
    let stored: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let expected = Status::ALL.map(Status::as_str);
    let mut out = Vec::new();
    for e in expected {
        if !stored.iter().any(|s| s == e) {
            out.push(format!("missing status '{e}'"));
        }
    }
    for s in &stored {
        if !expected.contains(&s.as_str()) {
            out.push(format!("unknown status '{s}'"));
        }
    }
    Ok(out)
}

fn row_sanity(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id, title, created_at, updated_at FROM issue")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, created, updated) = row?;
        if let Err(msg) = validate_title(&title) {
            out.push(format!("#{id}: {msg}"));
        }
        for (field, val) in [("created_at", &created), ("updated_at", &updated)] {
            if chrono::DateTime::parse_from_rfc3339(val).is_err() {
                out.push(format!("#{id}: {field} is not RFC 3339: '{val}'"));
            }
        }
    }
    Ok(out)
}

/// The version-independent checks, doubling as the upgrade pre-flight:
/// both pragmas validate whatever schema the file itself declares, so
/// they run on any schema version.
pub fn preflight(conn: &Connection) -> Result<bool> {
    let a = report("integrity_check", &integrity(conn)?);
    let b = report("foreign_key_check", &foreign_keys(conn)?);
    Ok(a && b)
}

/// The schema-aware checks, written against the current schema version
/// only; callers gate on the version before running them.
pub fn current_schema(conn: &Connection) -> Result<bool> {
    let a = report("status vocabulary", &vocabulary(conn)?);
    let b = report("row sanity", &row_sanity(conn)?);
    Ok(a && b)
}
