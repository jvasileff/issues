//! Integration tests (§13 of the plan). Interactive flows are driven with
//! ISSUES_ASSUME_TTY=1 and a scripted $EDITOR.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("issues")
}

fn cmd(dir: &Path) -> Command {
    let mut c = Command::new(bin());
    c.current_dir(dir);
    c.env_remove("VISUAL");
    c.env_remove("EDITOR");
    c.env_remove("ISSUES_ASSUME_TTY");
    c.env_remove("ISSUES_CRASH_BEFORE_COMMIT");
    c.env_remove("NO_COLOR");
    c
}

fn project() -> TempDir {
    let t = tempfile::tempdir().unwrap();
    cmd(t.path()).arg("init").assert().success();
    t
}

fn add(dir: &Path, title: &str, args: &[&str]) -> i64 {
    let out = cmd(dir).arg("add").arg(title).args(args).output().unwrap();
    assert!(out.status.success(), "add failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("created #")
        .unwrap()
        .parse()
        .unwrap()
}

fn stdout_of(dir: &Path, args: &[&str]) -> String {
    let out = cmd(dir).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn show(dir: &Path, id: i64) -> String {
    stdout_of(dir, &["show", &id.to_string()])
}

/// Body part of `show` output (everything after the closing `---` line).
fn body_of(dir: &Path, id: i64) -> String {
    let s = show(dir, id);
    let idx = s.find("\n---\n").expect("show output has no closing ---");
    s[idx + 5..].to_string()
}

/// A scripted $EDITOR: a shell script receiving the draft path as $1.
fn write_editor(dir: &Path, name: &str, script_body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{script_body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p.to_str().unwrap().to_string()
}

fn draft_path(dir: &Path, id: i64) -> PathBuf {
    dir.join(".issues/drafts").join(format!("{id}.md"))
}

// ---------------------------------------------------------------- §13.1

#[test]
fn init_add_list_show_roundtrip() {
    let t = project();
    // init is idempotent and prints the CLAUDE.md snippet
    cmd(t.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Add the following to your project's CLAUDE.md"))
        .stdout(predicate::str::contains("## Issue tracker"));
    assert_eq!(fs::read_to_string(t.path().join(".issues/.gitignore")).unwrap(), "*\n");

    let a = add(t.path(), "Open idea", &[]);
    let b = add(t.path(), "Finished work", &["--status", "done", "--body", "was done\n"]);

    let list = stdout_of(t.path(), &["list"]);
    assert!(list.contains("Open idea"));
    assert!(!list.contains("Finished work"), "done issues must be hidden by default");

    let all = stdout_of(t.path(), &["list", "--all"]);
    assert!(all.contains("Open idea") && all.contains("Finished work"));

    let only_done = stdout_of(t.path(), &["list", "--status", "done"]);
    assert!(!only_done.contains("Open idea") && only_done.contains("Finished work"));

    assert_eq!(show(t.path(), a), format!("---\nid: {a}\ntitle: \"Open idea\"\nstatus: idea\n---\n"));
    assert_eq!(
        show(t.path(), b),
        format!("---\nid: {b}\ntitle: \"Finished work\"\nstatus: done\n---\nwas done\n")
    );
}

#[test]
fn list_empty_messages() {
    let t = project();
    cmd(t.path()).arg("list").assert().success().stdout(predicate::str::contains("no open issues"));
    add(t.path(), "x", &[]);
    cmd(t.path())
        .args(["list", "--status", "done"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no issues match"));
}

// ---------------------------------------------------------------- §13.2

#[test]
fn stdin_bodies() {
    let t = project();
    let id = add(t.path(), "T", &["--body", "-"]);
    // "-" with no piped stdin content -> empty; redo with real stdin:
    let _ = id;
    let out = cmd(t.path())
        .args(["add", "Stdin body", "--body", "-"])
        .write_stdin("line one\nline two\n")
        .output()
        .unwrap();
    assert!(out.status.success());
    let id2: i64 = String::from_utf8(out.stdout).unwrap().trim().strip_prefix("created #").unwrap().parse().unwrap();
    assert_eq!(body_of(t.path(), id2), "line one\nline two\n");

    cmd(t.path())
        .args(["set-body", &id2.to_string(), "--body", "-"])
        .write_stdin("replaced\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id2} body updated")));
    assert_eq!(body_of(t.path(), id2), "replaced\n");
}

// ---------------------------------------------------------------- §13.3

#[test]
fn lenient_status_parsing() {
    let t = project();
    let id = add(t.path(), "T", &[]);
    cmd(t.path())
        .args(["update", &id.to_string(), "--status", "in_progress"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status: idea → in-progress"));
    assert!(show(t.path(), id).contains("status: in-progress\n"));

    cmd(t.path())
        .args(["update", &id.to_string(), "--status", "DONE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status: in-progress → done"));
    assert!(show(t.path(), id).contains("status: done\n"));

    cmd(t.path())
        .args(["update", &id.to_string(), "--status", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("idea, agreed, in-progress, done, abandoned"));
}

// ---------------------------------------------------------------- §13.4

#[test]
fn parent_child() {
    let t = project();
    let parent = add(t.path(), "Parent plan", &["--status", "agreed"]);
    let child = add(t.path(), "Child task", &["--parent", &parent.to_string()]);

    // parent displayed -> child indented beneath it
    let list = stdout_of(t.path(), &["list"]);
    let lines: Vec<&str> = list.lines().collect();
    let p_line = lines.iter().position(|l| l.contains("Parent plan")).unwrap();
    let c_line = lines.iter().position(|l| l.contains("  Child task")).unwrap();
    assert!(c_line == p_line + 1, "child must render indented right beneath its parent:\n{list}");
    assert!(!list.contains("(sub of"));

    // parent hidden (done) -> child flat with suffix
    cmd(t.path()).args(["update", &parent.to_string(), "--status", "done"]).assert().success();
    let list = stdout_of(t.path(), &["list"]);
    assert!(list.contains(&format!("Child task (sub of #{parent})")), "{list}");

    // --parent filter
    let filtered = stdout_of(t.path(), &["list", "--parent", &parent.to_string()]);
    assert!(filtered.contains("Child task"));

    // unset with --parent none
    cmd(t.path())
        .args(["update", &child.to_string(), "--parent", "none"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("parent: #{parent} → none")));
    let list = stdout_of(t.path(), &["list"]);
    assert!(!list.contains("(sub of"));
    assert!(show(t.path(), child).starts_with(&format!("---\nid: {child}\ntitle: \"Child task\"\nstatus: idea\n---\n")));

    // bad parent rejected
    cmd(t.path())
        .args(["update", &child.to_string(), "--parent", "999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("parent issue #999 not found"));
}

// ---------------------------------------------------------------- §13.5

#[test]
fn str_replace() {
    let t = project();
    let id = add(t.path(), "T", &["--body", "one\ntwo\nthree\ndup\ndup\n"]);
    let ids = id.to_string();

    // multi-line unique replacement
    cmd(t.path())
        .args(["str-replace", &ids, "--old", "one\ntwo", "--new", "ONE"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id} body updated")));
    assert_eq!(body_of(t.path(), id), "ONE\nthree\ndup\ndup\n");

    let before = body_of(t.path(), id);

    // zero matches: exact error, body byte-identical
    cmd(t.path())
        .args(["str-replace", &ids, "--old", "missing", "--new", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "--old not found in #{id} body; no changes made"
        )));
    assert_eq!(body_of(t.path(), id), before);

    // multiple matches: exact error, body byte-identical
    cmd(t.path())
        .args(["str-replace", &ids, "--old", "dup", "--new", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "--old matches 2 times in #{id} body; provide more context to make it unique; no changes made"
        )));
    assert_eq!(body_of(t.path(), id), before);

    // empty --new deletes
    cmd(t.path())
        .args(["str-replace", &ids, "--old", "ONE\n", "--new", ""])
        .assert()
        .success();
    assert_eq!(body_of(t.path(), id), "three\ndup\ndup\n");
}

// ---------------------------------------------------------------- §13.6

#[test]
fn grep() {
    let t = project();
    let a = add(t.path(), "Auth refresh", &["--body", "alpha one\nbeta two\ngamma three\n"]);
    let b = add(t.path(), "Done thing", &["--status", "done", "--body", "beta in a done issue\n"]);

    // body match with correct 1-based line number, grouped format
    let out = stdout_of(t.path(), &["grep", "beta"]);
    assert_eq!(out, format!("#{a} Auth refresh [idea]\n  2: beta two\n"));

    // title match
    let out = stdout_of(t.path(), &["grep", "refresh"]);
    assert!(out.contains(&format!("#{a} Auth refresh [idea]\n  title: Auth refresh\n")));

    // context lines use '-' separator
    let out = stdout_of(t.path(), &["grep", "beta", "-C", "1"]);
    assert!(out.contains("  1- alpha one\n  2: beta two\n  3- gamma three\n"), "{out}");

    // case-insensitive
    cmd(t.path()).args(["grep", "BETA"]).assert().success().stdout(predicate::str::contains("no matches"));
    let out = stdout_of(t.path(), &["grep", "-i", "BETA"]);
    assert!(out.contains("beta two"));

    // default scope excludes done; --all includes it (blank line between issues)
    assert!(!stdout_of(t.path(), &["grep", "beta"]).contains("Done thing"));
    let out = stdout_of(t.path(), &["grep", "--all", "beta"]);
    assert!(out.contains(&format!("#{b} Done thing [done]")));
    assert!(out.contains("\n\n"), "blank line between issues: {out:?}");

    // --status narrows
    let out = stdout_of(t.path(), &["grep", "--status", "done", "beta"]);
    assert!(out.contains("Done thing") && !out.contains("Auth refresh"));
}

// ---------------------------------------------------------------- §13.7

#[test]
fn read_windowing() {
    let t = project();
    let id = add(t.path(), "T", &["--body", "l1\nl2\nl3\nl4\nl5\n"]);
    let ids = id.to_string();

    let out = stdout_of(t.path(), &["read", &ids]);
    assert_eq!(out.lines().count(), 5);
    assert!(out.starts_with(&format!("{:>6}\tl1\n", 1)));

    // absolute numbering with offset/limit
    let out = stdout_of(t.path(), &["read", &ids, "--offset", "2", "--limit", "2"]);
    assert_eq!(out, format!("{:>6}\tl2\n{:>6}\tl3\n", 2, 3));

    // read's numbers agree with grep's
    let g = stdout_of(t.path(), &["grep", "l3"]);
    assert!(g.contains("  3: l3"));
    let r = stdout_of(t.path(), &["read", &ids, "--offset", "3", "--limit", "1"]);
    assert!(r.contains("     3\tl3"));

    // out-of-range offset: stderr note, empty stdout, exit 0
    cmd(t.path())
        .args(["read", &ids, "--offset", "99"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(format!("#{id} body has 5 lines")));
}

// ---------------------------------------------------------------- §13.8

#[test]
fn edit_clean_save() {
    let t = project();
    let id = add(t.path(), "Edit me", &["--body", "first line\nsecond line\n"]);
    let ed = write_editor(
        t.path(),
        "ed.sh",
        r#"sed -i 's/second line/edited line/' "$1"
sed -i 's/^status: idea$/status: agreed/' "$1""#,
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id} saved (status: agreed)")));
    let s = show(t.path(), id);
    assert!(s.contains("status: agreed\n") && s.contains("edited line\n"));
    assert!(!draft_path(t.path(), id).exists(), "draft trio must be gone after commit");
    assert!(!t.path().join(".issues/drafts").join(format!("{id}.base.md")).exists());
    assert!(!t.path().join(".issues/drafts").join(format!("{id}.meta")).exists());
}

#[test]
fn edit_no_change_abort() {
    let t = project();
    let id = add(t.path(), "Untouched", &["--body", "body\n"]);
    let ed = write_editor(t.path(), "ed.sh", ":");
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains("no changes"));
    assert!(!draft_path(t.path(), id).exists());
    assert_eq!(body_of(t.path(), id), "body\n");
}

#[test]
fn edit_parse_error_fixed_on_retry() {
    let t = project();
    let id = add(t.path(), "Fixable", &["--body", "body\n"]);
    let state = t.path().join("state");
    let cap = t.path().join("cap.txt");
    let ed = write_editor(
        t.path(),
        "ed.sh",
        &format!(
            r#"if [ -f "{state}" ]; then
  cp "$1" "{cap}"
  sed -i 's/^status: bogus$/status: done/' "$1"
else
  touch "{state}"
  sed -i 's/^status: idea$/status: bogus/' "$1"
fi"#,
            state = state.display(),
            cap = cap.display()
        ),
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id} saved (status: done)")));
    // second launch saw the ERROR block
    let captured = fs::read_to_string(&cap).unwrap();
    assert!(captured.starts_with("# ERROR: unknown status 'bogus' on line 4.\n"), "{captured}");
    assert!(captured.contains("# Valid: idea, agreed, in-progress, done, abandoned."));
    assert!(show(t.path(), id).contains("status: done\n"));
    assert!(!draft_path(t.path(), id).exists());
}

#[test]
fn edit_parse_error_abort_keeps_draft() {
    let t = project();
    let id = add(t.path(), "Aborted", &["--body", "body\n"]);
    let state = t.path().join("state");
    let ed = write_editor(
        t.path(),
        "ed.sh",
        &format!(
            r#"if [ -f "{state}" ]; then
  :
else
  touch "{state}"
  sed -i 's/^status: idea$/status: bogus/' "$1"
fi"#,
            state = state.display()
        ),
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "aborted; draft kept at .issues/drafts/{id}.md (see 'issues drafts')"
        )));
    assert!(draft_path(t.path(), id).exists());
    // db untouched
    assert!(show(t.path(), id).contains("status: idea\n"));
    // and `list` warns about the draft
    cmd(t.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("note: 1 unsaved draft exists (issues drafts)"));
}

// ---------------------------------------------------------------- §13.9

#[test]
fn edit_concurrent_clean_automerge() {
    let t = project();
    let id = add(t.path(), "Merge me", &["--body", "alpha\nbeta\ngamma\n"]);
    // While the "editor" is open, a second CLI call flips the status;
    // the editor itself only touches the body -> non-overlapping -> clean.
    let ed = write_editor(
        t.path(),
        "ed.sh",
        &format!(
            r#""{bin}" update {id} --status agreed >/dev/null
sed -i 's/beta/BETA/' "$1""#,
            bin = bin().display()
        ),
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains("merged concurrent changes automatically"))
        .stdout(predicate::str::contains(format!("#{id} saved (status: agreed)")));
    let s = show(t.path(), id);
    assert!(s.contains("status: agreed\n"), "concurrent status change kept: {s}");
    assert!(s.contains("BETA\n"), "editor's body change kept: {s}");
    assert!(!draft_path(t.path(), id).exists());
}

#[test]
fn edit_concurrent_conflict_roundtrip() {
    let t = project();
    let id = add(t.path(), "Conflict me", &["--body", "shared line\n"]);
    let state = t.path().join("state");
    let cap = t.path().join("cap.txt");
    // Run 1: a concurrent CLI call rewrites the same line the editor edits
    // -> conflict. Run 2: capture the conflict-markered file, then write a
    // fully resolved checkout.
    let ed = write_editor(
        t.path(),
        "ed.sh",
        &format!(
            r#"if [ -f "{state}" ]; then
  cp "$1" "{cap}"
  printf '%s\n' '---' 'title: Conflict me' 'status: agreed' '---' 'resolved line' > "$1"
else
  touch "{state}"
  "{bin}" str-replace {id} --old "shared line" --new "their line" >/dev/null
  sed -i 's/shared line/our line/' "$1"
fi"#,
            state = state.display(),
            cap = cap.display(),
            bin = bin().display()
        ),
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id} saved (status: agreed)")));
    let captured = fs::read_to_string(&cap).unwrap();
    assert!(captured.starts_with("# CONFLICT:"), "{captured}");
    assert!(captured.contains("<<<<<<<") && captured.contains(">>>>>>>"), "{captured}");
    assert!(captured.contains("our line") && captured.contains("their line"), "{captured}");
    let s = show(t.path(), id);
    assert!(s.contains("status: agreed\n"));
    assert_eq!(body_of(t.path(), id), "resolved line\n");
    assert!(!draft_path(t.path(), id).exists());
}

// ---------------------------------------------------------------- §13.10

#[test]
fn crash_leaves_recoverable_draft_and_resume_commits() {
    let t = project();
    let id = add(t.path(), "Crashy", &["--body", "original body\n"]);
    let ed = write_editor(t.path(), "ed.sh", r#"sed -i 's/original body/edited body/' "$1""#);
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .env("ISSUES_CRASH_BEFORE_COMMIT", "1")
        .assert()
        .failure();

    // the draft trio survived, and nothing hit the db
    let drafts_dir = t.path().join(".issues/drafts");
    assert!(draft_path(t.path(), id).exists());
    assert!(drafts_dir.join(format!("{id}.base.md")).exists());
    assert!(drafts_dir.join(format!("{id}.meta")).exists());
    assert_eq!(body_of(t.path(), id), "original body\n");

    // `drafts` lists it
    cmd(t.path())
        .arg("drafts")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id}  Crashy")));

    // `drafts --diff` shows the pending change
    let diff = stdout_of(t.path(), &["drafts", "--diff", &id.to_string()]);
    assert!(diff.contains("-original body") && diff.contains("+edited body"), "{diff}");

    // `edit` offers resume; resuming (with a no-op editor) commits the
    // draft against the stored base
    let noop = write_editor(t.path(), "noop.sh", ":");
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &noop)
        .write_stdin("r\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("an unsaved draft for #{id} exists")))
        .stdout(predicate::str::contains(format!("#{id} saved")));
    assert_eq!(body_of(t.path(), id), "edited body\n");
    assert!(!draft_path(t.path(), id).exists());
}

// ---------------------------------------------------------------- §13.11

#[test]
fn tty_guard() {
    let t = project();
    let id = add(t.path(), "Guarded", &[]);
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("'issues edit' is interactive and requires a TTY."))
        .stderr(predicate::str::contains("Scriptable alternatives:"))
        .stderr(predicate::str::contains(format!("issues set-body {id} --body -")));

    cmd(t.path())
        .args(["add", "x", "-e"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a TTY"));

    cmd(t.path())
        .args(["drafts", "--resume", &id.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a TTY"));

    cmd(t.path())
        .args(["drafts", "--discard", &id.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a TTY"));
}

// ------------------------------------------------- rendered `show` (issue #4)

#[test]
fn show_plain_is_byte_identical_to_piped_show() {
    let t = project();
    let id = add(t.path(), "Plain check", &["--body", "line one\n\n- bullet\n"]);
    let piped = show(t.path(), id);
    let plain = stdout_of(t.path(), &["show", &id.to_string(), "--plain"]);
    assert_eq!(plain, piped);
}

#[test]
fn show_renders_under_tty_hook() {
    let t = project();
    let id = add(t.path(), "Render me", &["--body", "## Heading\n\nsome **bold** text\n"]);
    let child_id = add(t.path(), "Child render", &["--parent", &id.to_string()]);
    let out = cmd(t.path())
        .env("ISSUES_ASSUME_TTY", "1")
        .args(["show", &id.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\x1b["), "expected ANSI escapes in rendered view: {s:?}");
    assert!(s.contains(&format!("#{id}")));
    assert!(!s.contains("---"), "rendered view must not contain front-matter delimiters: {s:?}");

    // a child issue's rendered header includes its parent
    let child = String::from_utf8(
        cmd(t.path())
            .env("ISSUES_ASSUME_TTY", "1")
            .args(["show", &child_id.to_string()])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(child.contains(&format!("parent: #{id}")));
}

#[test]
fn show_no_color_wins_over_tty_hook() {
    let t = project();
    let id = add(t.path(), "No color", &["--body", "**bold** body\n"]);
    let canonical = show(t.path(), id);
    let out = cmd(t.path())
        .env("ISSUES_ASSUME_TTY", "1")
        .env("NO_COLOR", "1")
        .args(["show", &id.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), canonical);
}

#[test]
fn show_plain_preserves_markdown_body_bytes() {
    let t = project();
    let body = "- bullet one\n- **bold** item\n\n```rust\nfn main() {}\n```\n";
    let out = cmd(t.path())
        .args(["add", "Markdown body", "--body", "-"])
        .write_stdin(body)
        .output()
        .unwrap();
    assert!(out.status.success());
    let id: i64 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("created #")
        .unwrap()
        .parse()
        .unwrap();
    let s = stdout_of(t.path(), &["show", &id.to_string(), "--plain"]);
    let idx = s.find("\n---\n").expect("plain show output has no closing ---");
    assert_eq!(&s[idx + 5..], body);
}

// ------------------------------------------- YAML-quoted titles (issue #8)

#[test]
fn title_is_yaml_quoted_in_canonical_output() {
    let t = project();
    let id = add(t.path(), "Formatted output: with a colon", &["--body", "b\n"]);
    // exact bytes: the title is a YAML double-quoted scalar, so the whole
    // front-matter block is valid YAML even with ': ' in the title
    assert_eq!(
        show(t.path(), id),
        format!("---\nid: {id}\ntitle: \"Formatted output: with a colon\"\nstatus: idea\n---\nb\n")
    );
}

#[test]
fn quoted_title_round_trips_through_edit() {
    let t = project();
    let title = r#"weird "quoted" \ title: yes"#;
    let id = add(t.path(), title, &["--body", "body\n"]);
    let s = show(t.path(), id);
    assert!(s.contains(r#"title: "weird \"quoted\" \\ title: yes""#), "{s}");
    // the edit flow parses the escaped title back; editor only touches the body
    let ed = write_editor(t.path(), "ed.sh", r#"sed -i 's/^body$/edited/' "$1""#);
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success();
    assert_eq!(body_of(t.path(), id), "edited\n");
    let list = stdout_of(t.path(), &["list"]);
    assert!(list.contains(title), "title must survive an edit round-trip: {list}");
}

#[test]
fn bare_title_still_accepted_on_parse() {
    let t = project();
    let id = add(t.path(), "Old style", &["--body", "b\n"]);
    let ed = write_editor(t.path(), "ed.sh", r#"sed -i 's/^title: .*$/title: Renamed bare/' "$1""#);
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success();
    assert!(stdout_of(t.path(), &["list"]).contains("Renamed bare"));
}

#[test]
fn malformed_quoted_title_reopens_editor() {
    let t = project();
    let id = add(t.path(), "Fix quotes", &["--body", "body\n"]);
    let state = t.path().join("state");
    let cap = t.path().join("cap.txt");
    let ed = write_editor(
        t.path(),
        "ed.sh",
        &format!(
            r#"if [ -f "{state}" ]; then
  cp "$1" "{cap}"
  sed -i 's/^title: .*$/title: "Fixed title"/' "$1"
else
  touch "{state}"
  sed -i 's/^title: .*$/title: "unterminated/' "$1"
fi"#,
            state = state.display(),
            cap = cap.display()
        ),
    );
    cmd(t.path())
        .args(["edit", &id.to_string()])
        .env("ISSUES_ASSUME_TTY", "1")
        .env("EDITOR", &ed)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("#{id} saved")));
    let captured = fs::read_to_string(&cap).unwrap();
    assert!(captured.starts_with("# ERROR: unterminated quoted title on line 3"), "{captured}");
    assert!(stdout_of(t.path(), &["list"]).contains("Fixed title"));
}

#[test]
fn multiline_and_empty_titles_rejected() {
    let t = project();
    cmd(t.path())
        .args(["add", "two\nlines"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("single line"));
    cmd(t.path())
        .args(["add", "   "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("empty"));
    let id = add(t.path(), "fine", &[]);
    cmd(t.path())
        .args(["update", &id.to_string(), "--title", "cr\rhere"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("single line"));
    // db untouched by any of the rejects
    let list = stdout_of(t.path(), &["list"]);
    assert!(list.contains("fine") && !list.contains("lines") && !list.contains("cr"));
}
