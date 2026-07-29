## Issue tracker

This project tracks plans/bugs/todos in a local db via the `issues` CLI.

- `issues list` - open items (idea/agreed/in-progress/doc). Run at session start to load current state; check before proposing new work.
- `issues show <id>` - full item with markdown body. Read the full plan before implementing it.
- `issues add "title" --status idea --body -` - body markdown on stdin; `--parent <id>` for subtasks.
- `issues grep <regex>` - search all open issues (`-i`, `-C n`, `--all`); output is ripgrep-style with `#id` headings.
- `issues read <id> --offset <line> --limit <n>` - windowed line-numbered body read (same line numbers as grep); use for large bodies.
- `issues update <id> --status <s>` - statuses: idea, agreed, in-progress, done, abandoned, doc.
- `issues str-replace <id> --old <text> --new <text>` - targeted body edit; `--old` must match exactly once (same rules as your file-edit tool).
- `issues set-body <id> --body -` - replace whole body from stdin.

Issue bodies are the authoritative specs: an implementation plan lives in its issue body and should be buildable from `issues show <id>` alone.

Workflow: file new plans as `idea`; set `agreed` only after the human has reviewed the issue body and explicitly OK'd it. Split big plans into child issues; set `in-progress` when you start, `done` when implemented, `abandoned` if we drop it. Status `doc` marks a living document (memory, spec, conventions) that stays open and is kept current instead of ever closing.

Done and abandoned issues are frozen history: never edit them to keep them current; decisions that supersede them belong in the new issue or commit that made the change. Status updates and edits the human explicitly asks for are fine.

Unprompted edits: the AI may edit issues that are part of the current discussion or task; for issues outside that scope, propose the change or file a new issue instead.

Do not reference issue ids in commit messages: the issue db is not part of the git repository, so commit messages must stand on their own.

Use plain `-` instead of em-dashes in issue bodies and other planning docs; humans editing them can't easily type em-dashes, and mixed dashes break `str-replace` and `grep`.

Never use `issues edit` (interactive; human-only). Read-only SQL on `.issues/issues.db` is OK; all writes go through the CLI.
