# Issues

*Project-local issue tracker for rapid AI-assisted development.*

Human/AI pair development generates a constant stream of plans, feature ideas,
bug notes, and todos. Full trackers (GitHub Issues, Jira) impose too much
ceremony for that pace, so the work products end up as ad-hoc markdown files
that rot. `issues` replaces them with a single SQLite database in the project
directory (`.issues/issues.db`) and one CLI binary used by **both** the human
and the AI:

- **The AI** uses the scriptable subcommands (`list`, `show`, `read`,
  `add`, `update`, `set-body`, `str-replace`, `grep`) to file agreed plans,
  track status, and consult outstanding work before proposing new work.
- **The human** uses the same binary plus the interactive `edit` command,
  which checks an issue out into `$EDITOR` (git-commit style) and writes it
  back safely — with durable drafts, crash recovery, and automatic 3-way
  merge when the AI modifies the issue mid-edit.

This is a v1 prototype: no web UI, no tags/priorities/comments, no JSON
output, no git sync. The database directory is kept out of git entirely
(`.issues/.gitignore` contains `*`).

## Install

Requires stable Rust. The built binary has no external runtime dependencies.

```sh
cargo install --path issues-cli
```

That installs the `issues` binary from the `issues-cli` crate.

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
| `agreed` | Human and AI have agreed this should happen |
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
| `issues init` | Create `.issues/` (db, drafts dir, `.gitignore`); prints the block to paste into CLAUDE.md / AGENTS.md |
| `issues instructions` | Print the working agreement for an AI assistant; what the pasted block points at |
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
| `issues check` | Read-only self-audit: schema version, integrity, foreign keys, status vocabulary, row sanity |
| `issues upgrade` | Migrate an older database to this binary's schema (pre-flight, backup, post-check) |

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
- If the issue changes concurrently while you edit (the AI flipping a status
  is the common case), non-overlapping changes merge automatically;
  overlapping changes reopen the editor with diff3-style conflict markers.

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

## Schema versions and upgrades

The database records a schema version, and a binary refuses any database it
does not match exactly: a newer database needs a newer binary, and an older
database is never migrated implicitly — every command errors and points at
`issues upgrade`, the only command that migrates. An upgrade:

1. runs the version-independent subset of `issues check`
   (`PRAGMA integrity_check`, `PRAGMA foreign_key_check`) as a pre-flight,
   aborting with the database untouched if it fails;
2. backs the database up to `.issues/issues.db.v<old>.bak` via
   `VACUUM INTO` — which captures committed data still in the WAL, unlike a
   raw file copy — and refuses to overwrite an existing backup;
3. migrates inside a single transaction (a failure rolls back, leaving the
   database at the old version);
4. finishes with the full `issues check` suite. On failure the backup path
   is printed with a restore instruction; nothing is restored
   automatically, and the backup is kept on success too.

`issues check` itself is a read-only self-audit you can run any time.

## Teaching your AI assistant to use it

`issues init` prints a three-line block to paste into whatever file your
assistant reads as project instructions (`CLAUDE.md` and equivalents). The
block does not carry the instructions itself; it points the assistant at
`issues instructions` at the start of every session, and that command prints
the usage instructions — the commands, the status workflow, and the editing
rules.

The indirection is the point: the instructions ship with the binary, so
upgrading `issues` updates what your assistant reads, in every project, with
no file to re-paste and nothing to keep in sync. `init` is idempotent, so
re-running it in an existing project just reprints the block.

## License

MIT — see [LICENSE](LICENSE).
