use std::collections::HashSet;
use std::fmt;

use crate::model::{Issue, STATUS_VALUES, Status};

/// Help comments appended inside the front-matter block for `edit` checkouts.
pub const EDIT_COMMENTS: &str = "\
# status: idea | agreed | in-progress | done | abandoned
# Save and close the editor to write back. Close without changes to abort.
";

/// Title rule (issue #8), shared by every write boundary: non-blank and
/// single-line. Single-line is what keeps the front-matter serialization
/// parseable line-by-line (and, quoted, valid YAML).
pub fn validate_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title may not be empty".to_string());
    }
    if title.contains('\n') || title.contains('\r') {
        return Err("title must be a single line (no newlines)".to_string());
    }
    Ok(())
}

/// YAML double-quoted scalar with the minimal escape set: titles are
/// single-line by rule, so only `\` and `"` ever need escaping. Always
/// quoting keeps one serialization form and makes the front-matter block
/// valid YAML for editor tooling.
pub fn quote_title(title: &str) -> String {
    let mut s = String::with_capacity(title.len() + 2);
    s.push('"');
    for c in title.chars() {
        if c == '\\' || c == '"' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

/// Inverse of `quote_title`, lenient in shape: a value starting with `"`
/// must be a well-formed double-quoted scalar (only `\"` and `\\` escapes,
/// nothing after the closing quote); any other value is taken verbatim, so
/// bare titles from old drafts and hand edits keep working.
pub fn unquote_title(val: &str) -> Result<String, String> {
    let Some(rest) = val.strip_prefix('"') else {
        return Ok(val.to_string());
    };
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.chars();
    loop {
        match chars.next() {
            None => return Err("unterminated quoted title".to_string()),
            Some('"') => {
                let extra = chars.as_str();
                return if extra.is_empty() {
                    Ok(out)
                } else {
                    Err(format!(
                        "unexpected text after closing title quote: '{extra}'"
                    ))
                };
            }
            Some('\\') => match chars.next() {
                Some(c @ ('\\' | '"')) => out.push(c),
                Some(c) => return Err(format!("unknown escape '\\{c}' in title")),
                None => return Err("unterminated quoted title".to_string()),
            },
            Some(c) => out.push(c),
        }
    }
}

const TITLE_HINT: &str = r#"Write the title as "..." (escape " and \ with a backslash), or bare; a bare title may not begin with '"'."#;

/// Serialize an issue in the canonical checkout format (§8.2): a strict,
/// hand-parsed front-matter block followed by the body verbatim.
pub fn render(issue: &Issue, with_comments: bool) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("id: {}\n", issue.id));
    s.push_str(&format!("title: {}\n", quote_title(&issue.title)));
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
                return Err(err(
                    1,
                    "file must begin with a '---' front-matter line".into(),
                    None,
                ));
            }
        } else if line == "---" {
            parsed.body = text[end..].to_string();
            closed = true;
            break;
        } else if line.trim().is_empty() || line.trim_start().starts_with('#') {
            // blank lines and comments inside the block are ignored
        } else {
            let Some((k, v)) = line.split_once(':') else {
                return Err(err(
                    lineno,
                    format!("expected 'key: value', got '{line}'"),
                    None,
                ));
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
                    let t = unquote_title(val)
                        .map_err(|msg| err(lineno, msg, Some(TITLE_HINT.to_string())))?;
                    if let Err(msg) = validate_title(&t) {
                        return Err(err(lineno, msg, None));
                    }
                    parsed.title = Some(t);
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
        return Err(err(
            1,
            "file is empty; expected a '---' front-matter block".into(),
            None,
        ));
    }
    if !closed {
        return Err(err(lineno, "missing closing '---' line".into(), None));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_forms() {
        assert_eq!(quote_title("a: b"), r#""a: b""#);
        assert_eq!(quote_title(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_title(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn quote_unquote_round_trip() {
        for t in [
            "plain",
            "colon: title",
            r#"has "quotes""#,
            r"back\slash",
            r#"both \" at once"#,
            " padded ",
        ] {
            assert_eq!(unquote_title(&quote_title(t)).unwrap(), t);
        }
    }

    #[test]
    fn unquote_bare_is_verbatim() {
        assert_eq!(unquote_title("no quotes: fine").unwrap(), "no quotes: fine");
        assert_eq!(unquote_title("inner \" quote").unwrap(), "inner \" quote");
    }

    #[test]
    fn unquote_rejects_malformed() {
        assert!(unquote_title("\"unterminated").is_err());
        assert!(unquote_title(r#""done" extra"#).is_err());
        assert!(unquote_title(r#""bad \n escape""#).is_err());
        assert!(unquote_title(r#""ends in backslash \"#).is_err());
    }

    #[test]
    fn title_rules() {
        assert!(validate_title("ok").is_ok());
        assert!(validate_title("").is_err());
        assert!(validate_title("   ").is_err());
        assert!(validate_title("two\nlines").is_err());
        assert!(validate_title("cr\rhere").is_err());
    }
}
