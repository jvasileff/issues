## Building (Claude)
This checkout is shared with the human's macOS host. The repo is a Cargo virtual workspace (root Cargo.toml; the crate lives in issues/), so cargo commands run from the repo root and target/ lives at the root. Always build/run with `CARGO_TARGET_DIR=target/linux` (binary: `target/linux/debug/issues`). Never write to `target/debug` or `target/release` - those are the Mac's builds, and overwriting them causes cross-platform "exec format error" breakage.

## Issue tracker
This project tracks plans/bugs/todos in a local db via the `issues` CLI (not markdown files, not GitHub).

- `issues list` — open items (idea/agreed/in-progress). Check this before proposing new work.
- `issues show <id>` — full item with markdown body.
- `issues add "title" --status agreed --body -` — body markdown on stdin; `--parent <id>` for subtasks.
- `issues grep <regex>` — search all open issues (`-i`, `-C n`, `--all`); output is ripgrep-style with `#id` headings.
- `issues read <id> --offset <line> --limit <n>` — windowed line-numbered body read (same line numbers as grep); use for large bodies.
- `issues update <id> --status <s>` — statuses: idea, agreed, in-progress, done, abandoned.
- `issues str-replace <id> --old <text> --new <text>` — targeted body edit; `--old` must match exactly once (same rules as your Edit tool).
- `issues set-body <id> --body -` — replace whole body from stdin.

Workflow: when we agree on a plan, file it (status `agreed`); split big plans into child issues; set `in-progress` when you start, `done` when implemented, `abandoned` if we drop it.                                                          Never use `issues edit` (interactive; human-only). Read-only SQL on `.issues/issues.db` is OK; all writes go through the CLI.
