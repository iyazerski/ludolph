use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use ludolph_engine::index::Index;
use ludolph_engine::session::Outcome;
use serde_json::{Value, json};

fn header(version: u32) -> Value {
    json!({"type": "session", "version": version, "id": "s1", "timestamp": "2026-05-01T10:00:00.000Z", "cwd": "/work/app"})
}

fn user(id: &str, content: Value, ts: i64) -> Value {
    json!({"type": "message", "id": id, "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z",
           "message": {"role": "user", "content": content, "timestamp": ts}})
}

fn assistant(id: &str, stop: &str, ts: i64) -> Value {
    json!({"type": "message", "id": id, "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z",
           "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}], "stopReason": stop, "timestamp": ts}})
}

fn tool_result(id: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z",
           "message": {"role": "toolResult", "toolCallId": "c1", "toolName": "bash", "content": [{"type": "text", "text": "out"}], "timestamp": 9}})
}

fn name(id: &str, name: &str) -> Value {
    json!({"type": "session_info", "id": id, "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z", "name": name})
}

fn lines(entries: &[Value]) -> String {
    entries.iter().map(|e| format!("{e}\n")).collect()
}

fn append(path: &Path, text: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// A sessions folder with one session file in it.
fn sessions(entries: &[Value]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("--work-app--/2026-05-01T10-00-00-000Z_s1.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, lines(entries)).unwrap();
    (dir, path)
}

#[test]
fn summarizes_a_session_file() {
    let image = json!({"type": "image", "data": "aGk=", "mimeType": "image/png"});
    let (dir, path) = sessions(&[
        header(3),
        user(
            "u1",
            json!([{"type": "text", "text": "Fix\n  the   build"}, image]),
            1,
        ),
        assistant("a1", "toolUse", 2),
        tool_result("t1"),
        name("n1", "Old title"),
        name("n2", "  "),
        json!({"type": "model_change", "id": "m1", "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z"}),
        user("u2", json!([image]), 3),
        user("u3", json!("x".repeat(250)), 4),
        assistant("a2", "stop", 5),
    ]);
    append(&path, "not json\n");
    let mut index = Index::default();
    assert_eq!(index.refresh(dir.path()).unwrap(), vec![path.clone()]);

    let summary = index.get(&path).unwrap();
    assert_eq!(summary.id, "s1");
    assert_eq!(summary.cwd, Path::new("/work/app"));
    assert_eq!(summary.created.to_rfc3339(), "2026-05-01T10:00:00+00:00");
    // An empty name clears the title, as in pi.
    assert_eq!(summary.title, None);
    assert_eq!(summary.first_prompt.as_deref(), Some("Fix the build"));
    assert_eq!(summary.last_prompt, Some(format!("{}…", "x".repeat(200))));
    assert_eq!(summary.last_activity.unwrap().timestamp_millis(), 5);
    assert_eq!(summary.outcome, Some(Outcome::Done));
    assert_eq!(summary.leaf.as_deref(), Some("a2"));
}

#[test]
fn outcome_follows_the_last_message() {
    let cases = [
        (assistant("a", "stop", 1), Outcome::Done),
        (assistant("a", "error", 1), Outcome::Failed),
        (assistant("a", "length", 1), Outcome::Failed),
        (assistant("a", "aborted", 1), Outcome::Aborted),
        (assistant("a", "toolUse", 1), Outcome::Unfinished),
        (tool_result("a"), Outcome::Unfinished),
        (user("a", json!("next"), 1), Outcome::Unfinished),
    ];
    for (last, want) in cases {
        let (dir, path) = sessions(&[header(3), user("u", json!("hi"), 0), last.clone()]);
        let mut index = Index::default();
        index.refresh(dir.path()).unwrap();
        assert_eq!(index.get(&path).unwrap().outcome, Some(want), "last {last}");
    }
}

#[test]
fn refresh_reads_appends_rewrites_and_removals() {
    let (dir, path) = sessions(&[header(2), user("u1", json!("first"), 1)]);
    let state = dir.path().join("index.json");
    let mut index = Index::default();
    index.refresh(dir.path()).unwrap();
    assert_eq!(index.refresh(dir.path()).unwrap(), Vec::<PathBuf>::new());

    // A line pi is still writing is read once it ends.
    let named = name("n1", "Fix the build").to_string();
    let (start, end) = named.split_at(10);
    append(&path, start);
    index.refresh(dir.path()).unwrap();
    assert_eq!(index.get(&path).unwrap().title, None);
    append(&path, &format!("{end}\n"));
    index.refresh(dir.path()).unwrap();
    assert_eq!(
        index.get(&path).unwrap().title.as_deref(),
        Some("Fix the build")
    );

    // A saved index picks up where it left off.
    index.save(&state).unwrap();
    let mut loaded = Index::load(&state).unwrap();
    assert_eq!(loaded, index);
    append(&path, &lines(&[assistant("a1", "stop", 2)]));
    assert_eq!(loaded.refresh(dir.path()).unwrap(), vec![path.clone()]);
    assert_eq!(loaded.get(&path).unwrap().outcome, Some(Outcome::Done));

    // pi migrating the file to a new version rewrites it whole, at any size.
    let migrated = lines(&[header(3), user("u1", json!("first"), 1)]);
    let before = std::fs::metadata(&path).unwrap().len();
    let padding = before as usize - migrated.len() - 1;
    std::fs::write(&path, format!("{migrated}{}\n", " ".repeat(padding))).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
    loaded.refresh(dir.path()).unwrap();
    let summary = loaded.get(&path).unwrap();
    assert_eq!(
        (summary.title.as_deref(), summary.outcome),
        (None, Some(Outcome::Unfinished))
    );

    // Files that aren't sessions yet are left out, and removed ones drop out.
    let draft = path.with_file_name("draft.jsonl");
    std::fs::write(&draft, "").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(loaded.refresh(dir.path()).unwrap(), vec![path]);
    assert!(loaded.is_empty());
}

#[test]
fn a_missing_or_older_index_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("index.json");
    assert!(Index::load(&state).unwrap().is_empty());
    std::fs::write(&state, r#"{"format": 0, "rows": []}"#).unwrap();
    assert!(Index::load(&state).unwrap().is_empty());
    let mut index = Index::default();
    assert!(
        index
            .refresh(&dir.path().join("no-sessions"))
            .unwrap()
            .is_empty()
    );
}
