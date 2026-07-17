mod checkout;
mod db;
mod drafts;
mod edit;
mod merge;
mod model;
mod output;

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::PathBuf;
use std::process;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use regex::RegexBuilder;
use rusqlite::Connection;

use crate::model::Status;

const CLAUDE_SNIPPET: &str = "\
## Issue tracker
This project tracks plans/bugs/todos in a local db via the `issues` CLI (not markdown files, not GitHub).
- `issues list` — open items (idea/agreed/in-progress). Check this before proposing new work.
- `issues show <id>` — full item with markdown body.
- `issues add \"title\" --status agreed --body -` — body markdown on stdin; `--parent <id>` for subtasks.
- `issues grep <regex>` — search all open issues (`-i`, `-C n`, `--all`); output is ripgrep-style with `#id` headings.
- `issues read <id> --offset <line> --limit <n>` — windowed line-numbered body read (same line numbers as grep); use for large bodies.
- `issues update <id> --status <s>` — statuses: idea, agreed, in-progress, done, abandoned.
- `issues str-replace <id> --old <text> --new <text>` — targeted body edit; `--old` must match exactly once (same rules as your Edit tool).
- `issues set-body <id> --body -` — replace whole body from stdin.
Workflow: when we agree on a plan, file it (status `agreed`); split big plans into child issues; set `in-progress` when you start, `done` when implemented, `abandoned` if we drop it.
Never use `issues edit` (interactive; human-only). Read-only SQL on `.issues/issues.db` is OK; all writes go through the CLI.
";

const STATUS_HELP: &str = "\
Statuses (lifecycle: idea -> agreed -> in-progress -> done; abandoned from anywhere;
transitions are not enforced — the vocabulary is the feature):
  idea         brainstorming; not yet agreed to
  agreed       human and Claude have agreed this should happen
  in-progress  actively being implemented
  done         implemented (and merged/landed as applicable)
  abandoned    deliberately not doing this

Status input is forgiving: case-insensitive, and '_' is accepted for '-'
(in_progress -> in-progress). The hyphenated lowercase form is canonical in
the database and all output.";

fn long_about() -> String {
    format!(
        "Project-local issue tracker for human + Claude Code collaboration.\n\n\
         Issues live in a single SQLite database at .issues/issues.db, found by\n\
         walking up from the current directory ('issues init' creates it). Both\n\
         the human and Claude Code use this same binary: the scriptable\n\
         subcommands (list, show, read, add, update, set-body, str-replace,\n\
         grep) for automation, and the interactive 'edit' subcommand — which\n\
         checks an issue out into $EDITOR, git-commit style — for humans.\n\n\
         {STATUS_HELP}\n\n\
         Multi-line bodies: 'add' and 'set-body' accept '--body -' to read the\n\
         markdown body from stdin.\n\n\
         Concurrency: the database is opened in WAL mode and every edit is\n\
         optimistically locked, so multiple Claude Code sessions plus a human\n\
         editor session can safely work at once."
    )
}

#[derive(Parser)]
#[command(
    name = "issues",
    version,
    about = "Project-local issue tracker for human + Claude Code collaboration",
    long_about = long_about(),
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

fn parse_status(s: &str) -> Result<Status, String> {
    Status::from_str(s)
}

/// Issue id, tolerating a copy-pasted leading '#' ("#12" -> 12).
fn parse_id(s: &str) -> Result<i64, String> {
    s.strip_prefix('#')
        .unwrap_or(s)
        .parse()
        .map_err(|_| format!("invalid issue id '{s}'"))
}

#[derive(Subcommand)]
enum Cmd {
    /// Create .issues/ in the current directory (database, drafts/, .gitignore)
    ///
    /// Creates the SQLite database .issues/issues.db, the .issues/drafts/
    /// directory used by the interactive edit flow, and a .issues/.gitignore
    /// containing '*' so the whole directory stays out of git. Idempotent:
    /// safe to run in an already-initialized project. Afterwards it prints a
    /// ~10-line snippet to add to the project's CLAUDE.md so Claude Code
    /// knows the tool exists.
    #[command(after_long_help = "Example:\n  issues init")]
    Init,

    /// List issues (default: open only — idea, agreed, in-progress)
    ///
    /// Shows one issue per line with id, status, title, and relative update
    /// time. By default only open issues (idea, agreed, in-progress) are
    /// shown — this is the anti-rot mechanism; done and abandoned items
    /// disappear from view. Sort order: in-progress first, then agreed, then
    /// idea (then done/abandoned under --all), newest-updated first within
    /// each group. A child issue is indented beneath its parent when both
    /// are displayed; otherwise it is shown flat with a '(sub of #N)'
    /// suffix. If unsaved drafts exist, a one-line notice follows the list.
    #[command(
        after_long_help = "Examples:\n  issues list\n  issues list --all\n  issues list --status done\n  issues list --parent 7"
    )]
    List {
        /// Show all issues, including done and abandoned
        #[arg(long)]
        all: bool,
        /// Show only issues with this status (idea|agreed|in-progress|done|abandoned)
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,
        /// Show only children of the given issue id
        #[arg(long, value_name = "ID", value_parser = parse_id)]
        parent: Option<i64>,
    },

    /// Print one issue in full: front-matter plus markdown body
    ///
    /// In a terminal, the issue is rendered for reading: a '#<id> <title>
    /// [<status>]' header followed by the body formatted as markdown
    /// (headers, bold, bullets, code blocks), wrapped to the terminal
    /// width. When stdout is piped or redirected, when NO_COLOR is set, or
    /// when --plain is given, output is the canonical serialization
    /// instead: a '---'-delimited front-matter block (id, title, status,
    /// parent when set) followed by the raw markdown body, byte-stable for
    /// scripts. The canonical form is the same format 'edit' checks out
    /// into $EDITOR. For a windowed, line-numbered view of a large body,
    /// use 'read' instead.
    #[command(
        after_long_help = "Examples:\n  issues show 42\n  issues show 42 --plain   # canonical serialization even in a terminal"
    )]
    Show {
        #[arg(value_parser = parse_id)]
        id: i64,
        /// Print the canonical serialization even in a terminal
        #[arg(long)]
        plain: bool,
    },

    /// Windowed, line-numbered read of an issue's body
    ///
    /// Prints body lines in `cat -n` style: right-aligned line number, tab,
    /// line text. Line numbers are absolute body line numbers (1-based)
    /// regardless of --offset, and are the same numbers 'grep' reports, so
    /// the workflow composes: grep finds a hit at line 120, 'read <id>
    /// --offset 110 --limit 30' shows the neighborhood, str-replace edits
    /// it. An out-of-range --offset prints a note with the body's line count
    /// on stderr and exits 0.
    #[command(
        after_long_help = "Examples:\n  issues read 42\n  issues read 42 --offset 110 --limit 30"
    )]
    Read {
        #[arg(value_parser = parse_id)]
        id: i64,
        /// 1-based body line to start from
        #[arg(long, default_value_t = 1, value_name = "N")]
        offset: usize,
        /// Maximum number of lines to print (default: the whole body)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// Create an issue
    ///
    /// Creates an issue with the given title (default status: idea). Pass
    /// '--body -' to read a multi-line markdown body from stdin — the main
    /// path for scripted/Claude use. '--parent <id>' files it as a subtask.
    /// '-e/--edit' opens the new issue in $EDITOR immediately (interactive;
    /// requires a TTY). Prints the new id as 'created #<id>'.
    #[command(
        after_long_help = "Examples:\n  issues add \"Fix auth token refresh\"\n  issues add \"Session hardening plan\" --status agreed --body -   # body from stdin\n  issues add \"Rotate refresh tokens\" --parent 57\n  issues add \"Design notes\" -e"
    )]
    Add {
        title: String,
        /// Initial status (default: idea)
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,
        /// Parent issue id (files this issue as a subtask)
        #[arg(long, value_name = "ID", value_parser = parse_id)]
        parent: Option<i64>,
        /// Body text, or '-' to read the markdown body from stdin
        #[arg(long)]
        body: Option<String>,
        /// Open the new issue in $EDITOR immediately (interactive)
        #[arg(short = 'e', long)]
        edit: bool,
    },

    /// Update title, status, and/or parent (scriptable)
    ///
    /// Metadata-only update; at least one flag is required. Use '--parent
    /// none' to detach a subtask from its parent. Bodies are changed with
    /// set-body, str-replace, or (for humans) edit. Prints a one-line
    /// confirmation of what changed.
    #[command(
        after_long_help = "Examples:\n  issues update 42 --status in-progress\n  issues update 42 --title \"Fix auth token refresh (v2)\"\n  issues update 42 --parent none"
    )]
    Update {
        #[arg(value_parser = parse_id)]
        id: i64,
        /// New status (idea|agreed|in-progress|done|abandoned; lenient input)
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,
        /// New title
        #[arg(long)]
        title: Option<String>,
        /// New parent issue id, or 'none' to clear the parent
        #[arg(long, value_name = "ID|none")]
        parent: Option<String>,
    },

    /// Replace an issue's whole body (scriptable)
    ///
    /// Pass '--body -' to read the new markdown body from stdin. This is the
    /// non-interactive path for writing a body from scratch; for targeted
    /// changes prefer str-replace.
    #[command(
        after_long_help = "Examples:\n  issues set-body 42 --body \"short note\"\n  issues set-body 42 --body -   # body from stdin"
    )]
    SetBody {
        #[arg(value_parser = parse_id)]
        id: i64,
        /// New body text, or '-' to read the markdown body from stdin
        #[arg(long)]
        body: String,
    },

    /// Targeted body edit: replace one exact occurrence of --old with --new
    ///
    /// --old must match the body exactly once, byte-for-byte including
    /// whitespace and newlines (pass multi-line text as a quoted shell
    /// argument). Zero matches or multiple matches fail with no changes
    /// made — add more surrounding context to make the match unique. --new
    /// may be empty to delete the matched text. Title/status/parent are
    /// untouched (use update). The read-match-write happens in a single
    /// transaction, so a concurrent writer cannot interleave.
    #[command(
        after_long_help = "Examples:\n  issues str-replace 42 --old \"returns 401\" --new \"returns 403\"\n  issues str-replace 42 --old \"- stale item\n\" --new \"\"   # delete a line"
    )]
    StrReplace {
        #[arg(value_parser = parse_id)]
        id: i64,
        /// Exact text to replace (must occur exactly once in the body)
        #[arg(long)]
        old: String,
        /// Replacement text (may be empty, deleting the matched text)
        #[arg(long)]
        new: String,
    },

    /// Regex search across issue titles and bodies
    ///
    /// The pattern is a Rust `regex`-crate regular expression. Scope
    /// defaults to open issues (idea, agreed, in-progress); --all or
    /// --status widen or narrow it, mirroring list. Output is grouped
    /// ripgrep-style with the issue in the filename position: a '#<id>
    /// <title> [<status>]' heading per matching issue, then 'title: ...'
    /// for title matches and '<lineno>: <line>' for body matches. Body line
    /// numbers are 1-based over the raw body — identical to the numbers
    /// 'read' displays. Context lines from -C use a '<lineno>-' separator.
    /// Prints 'no matches' and exits 0 when nothing matches.
    #[command(
        after_long_help = "Examples:\n  issues grep 'refresh token'\n  issues grep -i 'auth' -C 2\n  issues grep --all 'sqlite'\n  issues grep --status done 'migration'"
    )]
    Grep {
        /// Regular expression (Rust regex crate syntax)
        pattern: String,
        /// Search all issues, including done and abandoned
        #[arg(long)]
        all: bool,
        /// Search only issues with this status
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,
        /// Case-insensitive matching
        #[arg(short = 'i', long)]
        ignore_case: bool,
        /// Print N context lines around body matches
        #[arg(short = 'C', long, value_name = "N")]
        context: Option<usize>,
    },

    /// Edit an issue in $EDITOR (interactive; human-only)
    ///
    /// Checks the issue out to .issues/drafts/<id>.md as front-matter (id,
    /// title, status, parent) plus markdown body, and opens $VISUAL /
    /// $EDITOR / vi on it (EDITOR="code --wait" works). Save and close
    /// to write back; close without changes to abort.
    ///
    /// The draft is durable: your edited bytes are never deleted until the
    /// database write has committed. If the editor or machine dies
    /// mid-session, the draft survives — 'issues drafts' lists it and the
    /// next 'issues edit <id>' offers to resume, diff, or discard it. Parse
    /// errors in the front-matter reopen the editor with an explanatory
    /// '# ERROR:' comment (fix and save, or close unchanged to abort,
    /// keeping the draft). If the issue is modified concurrently while you
    /// edit (e.g. by Claude), non-overlapping changes are merged
    /// automatically via 'git merge-file' (git must be installed);
    /// overlapping changes reopen the editor with conflict markers to
    /// resolve.
    ///
    /// Requires a TTY. Scripts and Claude must use the scriptable commands
    /// (update, set-body, str-replace) instead.
    #[command(after_long_help = "Example:\n  issues edit 42")]
    Edit {
        #[arg(value_parser = parse_id)]
        id: i64,
    },

    /// List or manage unsaved edit drafts
    ///
    /// Bare 'issues drafts' lists drafts left behind by aborted or crashed
    /// edit sessions (issue id, title, age). --resume re-enters the edit
    /// flow on the draft (TTY required); --diff prints a unified diff
    /// between the draft and the current issue; --discard deletes the draft
    /// after a y/N confirmation (TTY required).
    #[command(
        after_long_help = "Examples:\n  issues drafts\n  issues drafts --diff 42\n  issues drafts --resume 42\n  issues drafts --discard 42"
    )]
    Drafts {
        /// Re-enter the edit flow starting from the draft for this issue
        #[arg(long, value_name = "ID", value_parser = parse_id)]
        resume: Option<i64>,
        /// Print a unified diff between the draft and the current db state
        #[arg(long, value_name = "ID", value_parser = parse_id)]
        diff: Option<i64>,
        /// Delete the draft for this issue (asks for confirmation)
        #[arg(long, value_name = "ID", value_parser = parse_id)]
        discard: Option<i64>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        process::exit(1);
    }
}

fn open_project() -> Result<(PathBuf, Connection)> {
    let root = db::find_root()?;
    let conn = db::open(&root)?;
    Ok((root, conn))
}

/// §7.12: interactive commands refuse to run without a TTY, printing
/// scriptable alternatives. `ISSUES_ASSUME_TTY=1` bypasses (test hook).
fn require_tty(what: &str, alternatives: &[String]) {
    if edit::tty_ok() {
        return;
    }
    eprintln!("error: '{what}' is interactive and requires a TTY.");
    eprintln!("Scriptable alternatives:");
    for a in alternatives {
        eprintln!("  {a}");
    }
    process::exit(1);
}

fn resolve_body(arg: Option<&str>) -> Result<String> {
    match arg {
        None => Ok(String::new()),
        Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("failed to read body from stdin")?;
            Ok(s)
        }
        Some(text) => Ok(text.to_string()),
    }
}

/// Whether `show` uses the rendered view: --plain and NO_COLOR force the
/// canonical serialization; otherwise render iff stdout is a TTY
/// (`ISSUES_ASSUME_TTY=1` counts as one — test hook).
fn show_rendered(plain: bool) -> bool {
    if plain || std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty()) {
        return false;
    }
    std::env::var("ISSUES_ASSUME_TTY").as_deref() == Ok("1") || std::io::stdout().is_terminal()
}

/// Print `text` through a pager (git-style) when stdout is a real TTY,
/// directly otherwise. Respects $PAGER (run via the shell, so it may carry
/// arguments; set-but-empty disables paging); defaults to `less -RFX` so
/// output shorter than one screen prints exactly as an unpaged run would.
/// Falls back to direct printing if the pager can't be spawned.
fn page_or_print(text: &str) {
    if !std::io::stdout().is_terminal() {
        print!("{text}");
        return;
    }
    let mut cmd = match std::env::var("PAGER") {
        Ok(p) if p.is_empty() => {
            print!("{text}");
            return;
        }
        Ok(p) => {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &p]);
            c
        }
        Err(_) => {
            let mut c = std::process::Command::new("less");
            c.args(["-R", "-F", "-X"]);
            c
        }
    };
    match cmd.stdin(std::process::Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                // ignore EPIPE: the user may quit the pager before EOF
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
        Err(_) => print!("{text}"),
    }
}

fn fmt_parent(p: Option<i64>) -> String {
    p.map(|v| format!("#{v}"))
        .unwrap_or_else(|| "none".to_string())
}

/// Scope shared by list and grep: --status wins, then --all, else open only.
fn in_scope(status: model::Status, all: bool, filter: Option<Status>) -> bool {
    match filter {
        Some(f) => status == f,
        None => all || status.is_open(),
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Init => {
            let cwd = std::env::current_dir()?;
            let existed = cwd.join(".issues").is_dir();
            db::init(&cwd)?;
            println!(
                "{} .issues/ (issues.db, drafts/, .gitignore)",
                if existed { "verified" } else { "initialized" }
            );
            println!();
            println!("Add the following to your project's CLAUDE.md:");
            println!();
            print!("{CLAUDE_SNIPPET}");
            Ok(())
        }

        Cmd::List {
            all,
            status,
            parent,
        } => {
            let (root, conn) = open_project()?;
            let mut issues = db::all_issues(&conn)?;
            issues.retain(|i| {
                in_scope(i.status, all, status) && parent.is_none_or(|p| i.parent_id == Some(p))
            });
            if issues.is_empty() {
                if !all && status.is_none() && parent.is_none() {
                    println!("no open issues");
                } else {
                    println!("no issues match");
                }
            } else {
                print!("{}", output::render_list(&issues));
            }
            let drafts = drafts::list_drafts(&root);
            match drafts.len() {
                0 => {}
                1 => println!("note: 1 unsaved draft exists (issues drafts)"),
                n => println!("note: {n} unsaved drafts exist (issues drafts)"),
            }
            Ok(())
        }

        Cmd::Show { id, plain } => {
            let (_, conn) = open_project()?;
            let issue = db::get_issue(&conn, id)?;
            if show_rendered(plain) {
                page_or_print(&output::render_show(&issue));
            } else {
                let text = checkout::render(&issue, false);
                if text.ends_with('\n') {
                    print!("{text}");
                } else {
                    println!("{text}");
                }
            }
            Ok(())
        }

        Cmd::Read { id, offset, limit } => {
            let (_, conn) = open_project()?;
            let issue = db::get_issue(&conn, id)?;
            if offset == 0 {
                bail!("--offset is 1-based; use 1 for the first line");
            }
            let lines: Vec<&str> = issue.body.lines().collect();
            if offset > lines.len() {
                eprintln!("#{id} body has {} lines", lines.len());
                return Ok(());
            }
            let end = limit.map_or(lines.len(), |l| {
                (offset - 1).saturating_add(l).min(lines.len())
            });
            let mut stdout = std::io::stdout().lock();
            for (n, line) in lines.iter().enumerate().take(end).skip(offset - 1) {
                writeln!(stdout, "{:>6}\t{}", n + 1, line)?;
            }
            Ok(())
        }

        Cmd::Add {
            title,
            status,
            parent,
            body,
            edit: edit_flag,
        } => {
            checkout::validate_title(&title).map_err(|m| anyhow!(m))?;
            if edit_flag {
                require_tty(
                    "issues add --edit",
                    &[format!(
                        "issues add \"{title}\" --status agreed --body -        (reads markdown body from stdin)"
                    )],
                );
            }
            let (root, mut conn) = open_project()?;
            if let Some(p) = parent
                && !db::issue_exists(&conn, p)?
            {
                bail!("parent issue #{p} not found");
            }
            let body_text = resolve_body(body.as_deref())?;
            let id = db::add_issue(
                &conn,
                &title,
                status.unwrap_or(Status::Idea),
                parent,
                &body_text,
            )?;
            println!("created #{id}");
            if edit_flag {
                edit::edit(&mut conn, &root, id)?;
            }
            Ok(())
        }

        Cmd::Update {
            id,
            status,
            title,
            parent,
        } => {
            if let Some(t) = title.as_deref() {
                checkout::validate_title(t).map_err(|m| anyhow!(m))?;
            }
            let (_, mut conn) = open_project()?;
            let parent_change: Option<Option<i64>> = match parent.as_deref() {
                None => None,
                Some(s) if s.eq_ignore_ascii_case("none") => Some(None),
                Some(s) => {
                    Some(Some(parse_id(s).map_err(|_| {
                        anyhow!("--parent must be an issue id or 'none'")
                    })?))
                }
            };
            if status.is_none() && title.is_none() && parent_change.is_none() {
                bail!("nothing to update; provide at least one of --status, --title, --parent");
            }
            if let Some(Some(p)) = parent_change {
                if p == id {
                    bail!("issue #{id} cannot be its own parent");
                }
                if !db::issue_exists(&conn, p)? {
                    bail!("parent issue #{p} not found");
                }
            }
            let mut changes: Vec<String> = Vec::new();
            db::modify_issue(&mut conn, id, |i| {
                if let Some(s) = status {
                    changes.push(format!("status: {} → {}", i.status, s));
                    i.status = s;
                }
                if let Some(t) = title.as_deref() {
                    changes.push(format!("title: {} → {}", i.title, t));
                    i.title = t.to_string();
                }
                if let Some(p) = parent_change {
                    changes.push(format!(
                        "parent: {} → {}",
                        fmt_parent(i.parent_id),
                        fmt_parent(p)
                    ));
                    i.parent_id = p;
                }
                Ok(())
            })?;
            println!("#{id} {}", changes.join(", "));
            Ok(())
        }

        Cmd::SetBody { id, body } => {
            let (_, mut conn) = open_project()?;
            let body_text = resolve_body(Some(&body))?;
            db::modify_issue(&mut conn, id, |i| {
                i.body = body_text;
                Ok(())
            })?;
            println!("#{id} body updated");
            Ok(())
        }

        Cmd::StrReplace { id, old, new } => {
            let (_, mut conn) = open_project()?;
            if old.is_empty() {
                bail!("--old must not be empty");
            }
            db::modify_issue(&mut conn, id, |i| match i.body.matches(&old).count() {
                0 => bail!("--old not found in #{id} body; no changes made"),
                1 => {
                    i.body = i.body.replacen(&old, &new, 1);
                    Ok(())
                }
                n => bail!(
                    "--old matches {n} times in #{id} body; provide more context to make it unique; no changes made"
                ),
            })?;
            println!("#{id} body updated");
            Ok(())
        }

        Cmd::Grep {
            pattern,
            all,
            status,
            ignore_case,
            context,
        } => {
            let (_, conn) = open_project()?;
            let re = RegexBuilder::new(&pattern)
                .case_insensitive(ignore_case)
                .build()
                .map_err(|e| anyhow!("invalid regex: {e}"))?;
            let issues = db::all_issues(&conn)?;
            let scoped: Vec<&model::Issue> = issues
                .iter()
                .filter(|i| in_scope(i.status, all, status))
                .collect();
            match output::render_grep(&scoped, &re, context.unwrap_or(0)) {
                Some(text) => print!("{text}"),
                None => println!("no matches"),
            }
            Ok(())
        }

        Cmd::Edit { id } => {
            require_tty(
                "issues edit",
                &[
                    format!("issues update {id} --status done --title \"...\""),
                    format!("issues str-replace {id} --old \"...\" --new \"...\""),
                    format!(
                        "issues set-body {id} --body -        (reads markdown body from stdin)"
                    ),
                ],
            );
            let (root, mut conn) = open_project()?;
            edit::edit(&mut conn, &root, id)
        }

        Cmd::Drafts {
            resume,
            diff,
            discard,
        } => {
            let picked = [resume.is_some(), diff.is_some(), discard.is_some()]
                .iter()
                .filter(|b| **b)
                .count();
            if picked > 1 {
                bail!("use only one of --resume, --diff, --discard");
            }
            if let Some(id) = resume {
                require_tty(
                    "issues drafts --resume",
                    &[
                        "issues drafts               (list drafts)".to_string(),
                        format!("issues drafts --diff {id}    (inspect the draft)"),
                    ],
                );
                let (root, mut conn) = open_project()?;
                return edit::resume_draft(&mut conn, &root, id);
            }
            if let Some(id) = discard {
                require_tty(
                    "issues drafts --discard",
                    &[
                        "issues drafts               (list drafts)".to_string(),
                        format!("issues drafts --diff {id}    (inspect the draft)"),
                    ],
                );
                let (root, _conn) = open_project()?;
                let paths = drafts::DraftPaths::new(&root, id);
                if !paths.draft.exists() {
                    bail!("no draft for #{id}");
                }
                print!("discard draft for #{id}? [y/N] ");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    paths.delete()?;
                    println!("draft for #{id} discarded");
                } else {
                    println!("draft kept");
                }
                return Ok(());
            }
            let (root, conn) = open_project()?;
            if let Some(id) = diff {
                return drafts::print_diff(&conn, &root, id);
            }
            let list = drafts::list_drafts(&root);
            if list.is_empty() {
                println!("no drafts");
                return Ok(());
            }
            for d in list {
                let title = db::get_issue(&conn, d.id)
                    .map(|i| i.title)
                    .unwrap_or_else(|_| "(unknown issue)".to_string());
                println!("#{}  {}  ({} old)", d.id, title, d.age);
            }
            Ok(())
        }
    }
}
