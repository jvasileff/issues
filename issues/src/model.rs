use std::fmt;
use std::str::FromStr;

pub const STATUS_VALUES: &str = "idea, agreed, in-progress, done, abandoned";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idea,
    Agreed,
    InProgress,
    Done,
    Abandoned,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Agreed => "agreed",
            Status::InProgress => "in-progress",
            Status::Done => "done",
            Status::Abandoned => "abandoned",
        }
    }

    /// Sort group for `list`: in-progress, agreed, idea, then the closed ones.
    pub fn group(self) -> u8 {
        match self {
            Status::InProgress => 0,
            Status::Agreed => 1,
            Status::Idea => 2,
            Status::Done => 3,
            Status::Abandoned => 4,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Status::Idea | Status::Agreed | Status::InProgress)
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
            _ => Err(format!("unknown status '{s}'; valid: {STATUS_VALUES}")),
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
