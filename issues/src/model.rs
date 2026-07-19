use std::fmt;
use std::str::FromStr;

pub const STATUS_VALUES: &str = "idea, agreed, in-progress, done, abandoned, doc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idea,
    Agreed,
    InProgress,
    Done,
    Abandoned,
    /// Living-document entry (memory, spec, conventions): sits outside the
    /// task lifecycle, counts as open, and is kept perpetually current
    /// rather than ever flowing to done/abandoned.
    Doc,
}

impl Status {
    /// Every status, in help/display order. Drives the statuses-table seed,
    /// so the enum stays the single source of truth for the vocabulary.
    pub const ALL: [Status; 6] = [
        Status::Idea,
        Status::Agreed,
        Status::InProgress,
        Status::Done,
        Status::Abandoned,
        Status::Doc,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Agreed => "agreed",
            Status::InProgress => "in-progress",
            Status::Done => "done",
            Status::Abandoned => "abandoned",
            Status::Doc => "doc",
        }
    }

    /// Sort group for `list`: actionable work first (in-progress, agreed,
    /// idea), then docs so they never crowd it, then the closed ones.
    pub fn group(self) -> u8 {
        match self {
            Status::InProgress => 0,
            Status::Agreed => 1,
            Status::Idea => 2,
            Status::Doc => 3,
            Status::Done => 4,
            Status::Abandoned => 5,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(
            self,
            Status::Idea | Status::Agreed | Status::InProgress | Status::Doc
        )
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Status {
    type Err = String;

    /// Lenient: case-insensitive, `_` accepted for `-`. The hyphenated
    /// lowercase form is canonical everywhere else.
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "idea" => Ok(Status::Idea),
            "agreed" => Ok(Status::Agreed),
            "in-progress" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            "abandoned" => Ok(Status::Abandoned),
            "doc" => Ok(Status::Doc),
            _ => Err(format!("unknown status '{s}'; valid: {STATUS_VALUES}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_values_lists_all() {
        assert_eq!(Status::ALL.map(Status::as_str).join(", "), STATUS_VALUES);
    }

    #[test]
    fn all_statuses_round_trip() {
        for s in Status::ALL {
            assert_eq!(s.as_str().parse::<Status>().unwrap(), s);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub id: i64,
    pub title: String,
    pub status: Status,
    pub body: String,
    pub parent_id: Option<i64>,
    /// Kept for round-trip completeness; not currently displayed anywhere.
    #[allow(dead_code)]
    pub created_at: String,
    pub updated_at: String,
}
