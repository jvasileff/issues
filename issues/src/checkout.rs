use std::collections::HashSet;
use std::fmt;

use crate::model::{Issue, STATUS_VALUES, Status};

/// Help comments appended inside the front-matter block for `edit` checkouts.
pub const EDIT_COMMENTS: &str = "\
# status: idea | agreed | in-progress | done | abandoned
# Save and close the editor to write back. Close without changes to abort.
";

/// Serialize an issue in the canonical checkout format (§8.2): a strict,
/// hand-parsed front-matter block followed by the body verbatim.
pub fn render(issue: &Issue, with_comments: bool) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("id: {}\n", issue.id));
    s.push_str(&format!("title: {}\n", issue.title));
    s.push_str(&format!("status: {}\n", issue.status));
    if let Some(p) = issue.parent_id {
        s.push_str(&format!("parent: {p}\n"));
    }
    if with_comments {
        s.push_str(EDIT_COMMENTS);
    }
    s.push_str("---\n");
    s.push_str(&issue.body);
    s
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// `None` means "unchanged from base".
    pub title: Option<String>,
    /// `None` means "unchanged from base".
    pub status: Option<Status>,
    /// `None` means no parent (the line is omitted, or `parent: none`).
    pub parent: Option<i64>,
    pub body: String,
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
    pub hint: Option<String>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on line {}", self.msg, self.line)
    }
}

pub fn parse(text: &str) -> Result<Parsed, ParseError> {
    fn err(line: usize, msg: String, hint: Option<String>) -> ParseError {
        ParseError { line, msg, hint }
    }

    let mut parsed = Parsed::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut lineno = 0usize;
    let mut offset = 0usize;
    let mut closed = false;

    for raw in text.split_inclusive('\n') {
        lineno += 1;
        let end = offset + raw.len();
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);

        if lineno == 1 {
            if line != "---" {
                return Err(err(1, "file must begin with a '---' front-matter line".into(), None));
            }
        } else if line == "---" {
            parsed.body = text[end..].to_string();
            closed = true;
            break;
        } else if line.trim().is_empty() || line.trim_start().starts_with('#') {
            // blank lines and comments inside the block are ignored
        } else {
            let Some((k, v)) = line.split_once(':') else {
                return Err(err(lineno, format!("expected 'key: value', got '{line}'"), None));
            };
            let key = k.trim().to_string();
            let val = v.trim();
            if !seen.insert(key.clone()) {
                return Err(err(lineno, format!("duplicate key '{key}'"), None));
            }
            match key.as_str() {
                // Informational only; never trusted for routing — the id
                // comes from the command line.
                "id" => {}
                "title" => {
                    if val.is_empty() {
                        return Err(err(lineno, "title may not be empty".into(), None));
                    }
                    parsed.title = Some(val.to_string());
                }
                "status" => match val.parse::<Status>() {
                    Ok(s) => parsed.status = Some(s),
                    Err(_) => {
                        return Err(err(
                            lineno,
                            format!("unknown status '{val}'"),
                            Some(format!("Valid: {STATUS_VALUES}.")),
                        ));
                    }
                },
                "parent" => {
                    if !val.eq_ignore_ascii_case("none") {
                        match val.parse::<i64>() {
                            Ok(p) => parsed.parent = Some(p),
                            Err(_) => {
                                return Err(err(
                                    lineno,
                                    format!("parent must be an issue id or 'none', got '{val}'"),
                                    None,
                                ));
                            }
                        }
                    }
                }
                other => return Err(err(lineno, format!("unknown key '{other}'"), None)),
            }
        }
        offset = end;
    }

    if lineno == 0 {
        return Err(err(1, "file is empty; expected a '---' front-matter block".into(), None));
    }
    if !closed {
        return Err(err(lineno, "missing closing '---' line".into(), None));
    }
    Ok(parsed)
}
