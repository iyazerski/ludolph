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
fn spend_includes_all_usage_and_survives_a_restart() {
    let usage = json!({"input": 100, "output": 20, "cacheRead": 40, "cacheWrite": 8,
                       "totalTokens": 168, "cost": {"total": 0.125}});
    let mut answer = assistant("a1", "aborted", 2);
    answer["message"]["usage"] = usage.clone();
    answer["message"]["provider"] = json!("anthropic");
    answer["message"]["model"] = json!("opus");
    let mut result = tool_result("t1");
    result["message"]["usage"] = usage.clone();
    let (dir, path) = sessions(&[header(3), user("u1", json!("hi"), 1), answer, result]);
    let mut index = Index::default();
    index.refresh(dir.path()).unwrap();
    assert_eq!(index.get(&path).unwrap().spent.tokens, 336);
    assert_eq!(index.get(&path).unwrap().spent.cost, 0.25);
    let mut next = assistant("a2", "stop", 3);
    next["message"]["provider"] = json!("openai");
    next["message"]["model"] = json!("gpt");
    next["message"]["usage"] = json!({"input": 200, "output": 50,
        "cacheRead": 100, "cacheWrite": 10, "cost": {"total": 0.01}});
    let mut entries = vec![
        json!({"type": "model_change", "provider": "openai", "modelId": "gpt"}),
        next,
    ];
    entries.extend(
        ["compaction", "branch_summary", "usage"]
            .into_iter()
            .map(|kind| json!({"type": kind, "usage": usage})),
    );
    append(&path, &lines(&entries));
    index.refresh(dir.path()).unwrap();
    let summary = index.get(&path).unwrap();
    assert_eq!(summary.model.as_deref(), Some("openai/gpt"));
    assert_eq!(summary.spent.tokens, 1200);
    assert!((summary.spent.cost - 0.635).abs() < 1e-10);
    assert_eq!(summary.context_tokens, None);
    let state = dir.path().join("index.json");
    index.save(&state).unwrap();
    let mut loaded = Index::load(&state).unwrap();
    loaded.refresh(dir.path()).unwrap();
    assert_eq!(loaded.get(&path).unwrap().spent, summary.spent);
}

#[test]
fn refresh_reads_appends_rewrites_and_removals() {
    let (dir, path) = sessions(&[header(2), user("u1", json!("first"), 1)]);
    let state = dir.path().join("index.json");
    let mut index = Index::default();
    index.refresh(dir.path()).unwrap();
    assert_eq!(index.refresh(dir.path()).unwrap(), Vec::<PathBuf>::new());

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

    index.save(&state).unwrap();
    let mut loaded = Index::load(&state).unwrap();
    assert_eq!(loaded, index);
    append(&path, &lines(&[assistant("a1", "stop", 2)]));
    assert_eq!(loaded.refresh(dir.path()).unwrap(), vec![path.clone()]);
    assert_eq!(loaded.get(&path).unwrap().outcome, Some(Outcome::Done));

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

    let draft = path.with_file_name("draft.jsonl");
    std::fs::write(&draft, "").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(loaded.refresh(dir.path()).unwrap(), vec![path]);
    assert!(loaded.is_empty());
}
