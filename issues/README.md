# `issues` — project-local issue tracker for human + Claude Code collaboration

Human/AI pair development generates a constant stream of plans, feature ideas,
bug notes, and todos. Full trackers (GitHub Issues, Jira) impose too much
ceremony for that pace, so the work products end up as ad-hoc markdown files
that rot. `issues` replaces them with a single SQLite database in the project
directory (`.issues/issues.db`) and one CLI binary used by **both** the human
and Claude Code:

- **Claude Code** uses the scriptable subcommands (`list`, `show`, `read`,
  `add`, `update`, `set-body`, `str-replace`, `grep`) to file agreed plans,
  track status, and consult outstanding work before proposing new work.
- **The human** uses the same binary plus the interactive `edit` command,
  which checks an issue out into `$EDITOR` (git-commit style) and writes it
  back safely — with durable drafts, crash recovery, and automatic 3-way
  merge when Claude modifies the issue mid-edit.

This is a v1 prototype: no web UI, no tags/priorities/comments, no JSON
output, no git sync. The database directory is kept out of git entirely
(`.issues/.gitignore` contains `*`).

## Install

Requires stable Rust and — for the `edit` merge path — `git` on PATH.

```sh
cargo install --path .
```

## Quick start

```sh
issues init                                  # creates .issues/ in the current directory
issues add "Fix auth token refresh"          # -> created #1
issues add "Session plan" --status agreed --body -   # multi-line body from stdin
issues list                                  # open items only (idea/agreed/in-progress/doc)
issues edit 1                                # human: full edit in $EDITOR
issues update 1 --status done                # done items disappear from `issues list`
```

Every other command locates the project root by walking up from the current
directory until it finds `.issues/`.

## Statuses

`idea → agreed → in-progress → done`, with `abandoned` reachable from
anywhere and `doc` sitting outside the lifecycle entirely. Transitions are
**not** enforced — the vocabulary is the feature.

| Status | Meaning |
|---|---|
| `idea` | Brainstorming; not yet agreed to |
| `agreed` | Human and Claude have agreed this should happen |
| `in-progress` | Actively being implemented |
| `done` | Implemented (and merged/landed as applicable) |
| `abandoned` | Deliberately not doing this |
| `doc` | Living document (memory, spec, conventions); stays open, never closes |

`done` and `abandoned` bodies are frozen history; `doc` bodies are the
opposite, perpetually edited to stay current. In `list`, docs form their own
group after the actionable statuses so they never crowd active work.

Status input is forgiving everywhere (case-insensitive, `_` accepted for
`-`); the hyphenated lowercase form is canonical.

## Command tour

Run `issues --help` and `issues <cmd> --help` for the full reference — the
help text is intentionally exhaustive.

| Command | Purpose |
|---|---|
| `issues init` | Create `.issues/` (db, drafts dir, `.gitignore`); prints the CLAUDE.md snippet |
| `issues list [--all] [--status s] [--parent id]` | Aligned listing; open issues only by default (the anti-rot mechanism); children indent beneath their parent |
| `issues show <id>` | Full issue: front-matter + markdown body |
| `issues read <id> [--offset n] [--limit n]` | Windowed, line-numbered body read; same line numbers as `grep` |
| `issues add <title> [--status s] [--parent id] [--body text\|-]` | Create; `--body -` reads stdin; `-e` opens `$EDITOR` |
| `issues update <id> [--status s] [--title t] [--parent id\|none]` | Scriptable metadata update |
| `issues set-body <id> --body <text\|->` | Replace the whole body |
| `issues str-replace <id> --old <text> --new <text>` | Targeted body edit; `--old` must match exactly once |
| `issues grep <regex> [--all] [--status s] [-i] [-C n]` | ripgrep-style grouped search across titles and bodies |
| `issues edit <id>` | Interactive `$EDITOR` flow (human-only; requires a TTY) |
| `issues drafts [--resume id \| --diff id \| --discard id]` | List/manage unsaved edit drafts |

In a terminal, `issues show` renders the issue for reading: a bold header
plus the body formatted as markdown (headers, bullets, code blocks), wrapped
to the terminal width. When piped or redirected, when `NO_COLOR` is set, or
with `--plain`, it prints the canonical serialization instead — front-matter
plus raw body, byte-stable for scripts.

A composable research loop for large plan documents: `grep` finds a hit at
line 120 → `read 42 --offset 110 --limit 30` shows the neighborhood →
`str-replace` edits it.

## The `edit` flow and draft durability

`issues edit 42` writes the issue to `.issues/drafts/42.md` as a
front-matter block (id, title, status, parent) plus raw markdown body, and
opens your editor on it. Save and close to write back; close without changes
to abort.

The core invariant: **your edited bytes always exist in at least one durable
place** — the draft file is never deleted until the database transaction has
committed. Consequences:

- Killing the editor (or the machine) mid-session leaves a recoverable
  draft; `issues drafts` lists it and the next `issues edit 42` offers
  resume / diff / discard.
- Front-matter typos reopen the editor with a `# ERROR:` comment explaining
  the problem; fix and save, or close unchanged to abort (draft kept).
- If the issue changes concurrently while you edit (Claude flipping a status
  is the common case), non-overlapping changes merge automatically via
  `git merge-file`; overlapping changes reopen the editor with conflict
  markers.

The front matter looks like YAML so editors highlight it, but it is a strict
line format with no quoting rules: values run to the end of the line
verbatim, so titles like `fix #42: revised [draft]` round-trip byte-exact.

### Editor setup

The editor is `$VISUAL`, then `$EDITOR`, then `vi`, invoked via `sh -c`, so
multi-word values work unmodified:

```sh
EDITOR="code --wait" issues edit 42    # VS Code
EDITOR="vim" issues edit 42
```

## Direct SQL access

Read-only ad-hoc queries via the `sqlite3` CLI are fine, e.g.:

```sh
sqlite3 .issues/issues.db "SELECT id, title FROM issue WHERE status='abandoned'"
```

All **writes** must go through the `issues` CLI so timestamps and edit-flow
invariants hold. If other tooling ever does write, it must run
`PRAGMA foreign_keys=ON` first: SQLite leaves foreign keys unenforced by
default, and the schema relies on them (`issue.status` references the
`status` vocabulary table, whose rows change only with a schema-version
bump; `issue.parent_id` references `issue.id`). The CLI always sets the
pragma on its own connections.

## CLAUDE.md snippet

`issues init` prints this block; add it to your project's `CLAUDE.md`:

```markdown
## Issue tracker
This project tracks plans/bugs/todos in a local db via the `issues` CLI (not markdown files, not GitHub).
- `issues list` — open items (idea/agreed/in-progress/doc). Check this before proposing new work.
- `issues show <id>` — full item with markdown body.
- `issues add "title" --status agreed --body -` — body markdown on stdin; `--parent <id>` for subtasks.
- `issues grep <regex>` — search all open issues (`-i`, `-C n`, `--all`); output is ripgrep-style with `#id` headings.
- `issues read <id> --offset <line> --limit <n>` — windowed line-numbered body read (same line numbers as grep); use for large bodies.
- `issues update <id> --status <s>` — statuses: idea, agreed, in-progress, done, abandoned, doc.
- `issues str-replace <id> --old <text> --new <text>` — targeted body edit; `--old` must match exactly once (same rules as your Edit tool).
- `issues set-body <id> --body -` — replace whole body from stdin.
Workflow: when we agree on a plan, file it (status `agreed`); split big plans into child issues; set `in-progress` when you start, `done` when implemented, `abandoned` if we drop it. Status `doc` marks a living document (memory, spec, conventions) that stays open and is kept current instead of ever closing.
Never use `issues edit` (interactive; human-only). Read-only SQL on `.issues/issues.db` is OK; all writes go through the CLI.
```
