use std::io::Write;
use std::path::Path;

use ludolph_engine::transcript::{
    Image, Message, MessageStatus, Page, Part, Role, Tail, ToolState, Transcripts, Window,
};
use serde_json::{Value, json};

const HEADER: &str = r#"{"type":"session","version":3,"id":"s1","timestamp":"2026-05-01T10:00:00.000Z","cwd":"/tmp"}"#;

/// Append entries to a session file.
fn append(path: &Path, entries: &[Value]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for entry in entries {
        writeln!(file, "{entry}").unwrap();
    }
}

/// A message entry `id`, child of `parent`.
fn message(id: &str, parent: Option<&str>, message: Value) -> Value {
    json!({
        "type": "message", "id": id, "parentId": parent,
        "timestamp": "2026-05-01T10:00:00.000Z", "message": message,
    })
}

fn user(text: &str, time: i64) -> Value {
    json!({"role": "user", "content": text, "timestamp": time})
}

fn bash(id: &str, command: &str) -> Value {
    json!({"type": "toolCall", "id": id, "name": "bash", "arguments": {"command": command}})
}

fn text(text: &str) -> Part {
    Part::Text { text: text.into() }
}

fn tool(id: &str, name: &str, subject: &str, state: ToolState) -> Part {
    Part::Tool {
        id: id.into(),
        name: name.into(),
        subject: Some(subject.into()),
        state,
    }
}

/// The page's messages as role, status and parts.
fn shown(page: &Page) -> Vec<(Role, MessageStatus, Vec<Part>)> {
    page.messages
        .iter()
        .map(|m: &Message| (m.role, m.status, m.parts.clone()))
        .collect()
}

#[test]
fn a_transcript_follows_the_leafs_branch_and_groups_turns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.jsonl");
    std::fs::write(&path, format!("{HEADER}\n")).unwrap();
    let read = json!({"type": "toolCall", "id": "c3", "name": "read", "arguments": {"path": "src/a.rs", "content": "big"}});
    append(
        &path,
        &[
            message(
                "sys",
                None,
                json!({"role": "system", "content": "", "timestamp": 1}),
            ),
            message("u1", Some("sys"), user("fix it", 2)),
            message(
                "a1",
                Some("u1"),
                json!({
                    "role": "assistant", "stopReason": "toolUse", "timestamp": 3,
                    "content": [
                        {"type": "thinking", "thinking": "hmm"},
                        {"type": "text", "text": "Looking."},
                        bash("c1", "cargo test"),
                        bash("c2", "ls"),
                    ],
                }),
            ),
            message(
                "r1",
                Some("a1"),
                json!({"role": "toolResult", "toolCallId": "c1", "toolName": "bash", "isError": true, "content": [{"type": "text", "text": "1 failed"}], "timestamp": 4}),
            ),
            message(
                "r2",
                Some("r1"),
                json!({"role": "toolResult", "toolCallId": "c2", "content": [], "timestamp": 5}),
            ),
            // An abandoned branch from u1, which the leaf's path doesn't take.
            message(
                "x1",
                Some("u1"),
                json!({"role": "assistant", "content": [{"type": "text", "text": "old"}], "stopReason": "stop", "timestamp": 6}),
            ),
            message(
                "a2",
                Some("r2"),
                json!({
                    "role": "assistant", "stopReason": "error", "errorMessage": "overloaded", "timestamp": 7,
                    "content": [read],
                }),
            ),
            json!({"type": "model_change", "id": "m1", "parentId": "a2", "timestamp": "2026-05-01T10:00:00.000Z", "provider": "p", "modelId": "m"}),
            json!({"type": "compaction", "id": "k1", "parentId": "m1", "timestamp": "2026-05-01T10:00:01.000Z", "summary": "…", "firstKeptEntryId": "a2", "tokensBefore": 9}),
            message(
                "b1",
                Some("k1"),
                json!({"role": "bashExecution", "command": "make", "output": "", "exitCode": 2, "cancelled": false, "truncated": false, "timestamp": 8}),
            ),
            message(
                "u2",
                Some("b1"),
                json!({
                    "role": "user", "timestamp": 9,
                    "content": [{"type": "text", "text": "see"}, {"type": "image", "data": "AAAA", "mimeType": "image/png"}],
                }),
            ),
        ],
    );
    // A lone surrogate, as pi writes half an emoji, which serde can't build.
    let cut = r#"{"type":"message","id":"a3","parentId":"u2","timestamp":"2026-05-01T10:00:00.000Z","message":{"role":"assistant","stopReason":"aborted","timestamp":10,"content":[{"type":"text","text":"cut \ud83d"}]}}"#;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{cut}").unwrap();

    let mut transcripts = Transcripts::default();
    let page = transcripts
        .page(Some(&path), Tail::default(), &Window::Latest(40))
        .unwrap();
    assert_eq!(
        shown(&page),
        [
            (Role::User, MessageStatus::Complete, vec![text("fix it")]),
            (
                Role::Assistant,
                MessageStatus::Complete,
                vec![
                    Part::Thinking { text: "hmm".into() },
                    text("Looking."),
                    tool("c1", "bash", "cargo test", ToolState::Failed),
                    tool("c2", "bash", "ls", ToolState::Done),
                    tool("c3", "read", "src/a.rs", ToolState::Done),
                    Part::Error {
                        error: "overloaded".into()
                    },
                ]
            ),
            (
                Role::System,
                MessageStatus::Complete,
                vec![text("Compacted the conversation.")]
            ),
            (
                Role::User,
                MessageStatus::Complete,
                vec![tool("b1", "bash", "make", ToolState::Failed)]
            ),
            (
                Role::User,
                MessageStatus::Complete,
                vec![text("see"), Part::Image { index: 0 }]
            ),
            (
                Role::Assistant,
                MessageStatus::Aborted,
                vec![text("cut \u{fffd}")]
            ),
        ]
    );
    assert_eq!(page.messages[1].id, "a1");
    assert_eq!(page.messages[1].created_at, 3);
    assert_eq!(page.leaf.as_deref(), Some("a3"));

    // A call's details load on request: its arguments and output, and a
    // command you ran yourself.
    let failed = transcripts.tool(&path, "c1").unwrap().unwrap();
    assert_eq!(failed.name, "bash");
    assert_eq!(failed.args, json!({"command": "cargo test"}));
    assert_eq!(
        (failed.text.as_str(), failed.failed, failed.running),
        ("1 failed", true, false)
    );
    let make = transcripts.tool(&path, "b1").unwrap().unwrap();
    assert_eq!((make.args, make.failed), (json!({"command": "make"}), true));
    assert_eq!(transcripts.tool(&path, "c3").unwrap(), None);
    let image = Image {
        mime_type: "image/png".into(),
        data: "AAAA".into(),
    };
    assert_eq!(transcripts.image(&path, "u2", 0).unwrap(), Some(image));
    assert_eq!(transcripts.image(&path, "u2", 1).unwrap(), None);

    // A page has the last messages, and what pi appends shows next time.
    append(&path, &[message("u3", Some("a3"), user("next", 11))]);
    let page = transcripts
        .page(Some(&path), Tail::default(), &Window::Latest(2))
        .unwrap();
    assert_eq!((page.total, page.older()), (7, 5));
    assert_eq!(shown(&page)[1].2, [text("next")]);

    // Anchored on a message, a page keeps it as more come, and can reach
    // back from it.
    let from = |id: &str, earlier| Window::From {
        id: id.into(),
        earlier,
    };
    append(&path, &[message("u4", Some("u3"), user("more", 12))]);
    let page = transcripts
        .page(Some(&path), Tail::default(), &from("u3", 0))
        .unwrap();
    let ids: Vec<&str> = page.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!((ids, page.older()), (vec!["u3", "u4"], 6));
    let page = transcripts
        .page(Some(&path), Tail::default(), &from("u3", 2))
        .unwrap();
    assert_eq!((page.messages.len(), page.older()), (4, 4));
    // An anchor that's gone gives the latest page.
    let page = transcripts
        .page(Some(&path), Tail::default(), &from("nope", 2))
        .unwrap();
    assert_eq!((page.messages.len(), page.older()), (8, 0));
}

#[test]
fn the_live_tail_adds_what_the_file_lacks_to_the_running_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.jsonl");
    std::fs::write(&path, format!("{HEADER}\n")).unwrap();
    let first = json!({"role": "assistant", "stopReason": "toolUse", "timestamp": 3, "content": [bash("c1", "ls")]});
    let result = json!({"role": "toolResult", "toolCallId": "c1", "content": [], "timestamp": 4});
    // pi wrote the prompt and the first reply, but not the result yet.
    append(
        &path,
        &[
            message("u1", None, user("go", 2)),
            message("a1", Some("u1"), first.clone()),
        ],
    );
    let second = json!({"role": "assistant", "stopReason": "toolUse", "timestamp": 5, "content": [bash("c2", "make")]});
    let streaming = json!({"role": "assistant", "stopReason": "pending", "timestamp": 6, "content": [{"type": "text", "text": "Buil"}]});
    let messages = [user("go", 2), first, result, second];
    let tail = Tail {
        messages: &messages,
        streaming: Some(&streaming),
    };

    let mut transcripts = Transcripts::default();
    let page = transcripts
        .page(Some(&path), tail, &Window::Latest(40))
        .unwrap();
    assert_eq!(
        shown(&page),
        [
            (Role::User, MessageStatus::Complete, vec![text("go")]),
            (
                Role::Assistant,
                MessageStatus::Streaming,
                vec![
                    tool("c1", "bash", "ls", ToolState::Done),
                    tool("c2", "bash", "make", ToolState::Running),
                    text("Buil"),
                ]
            ),
        ]
    );
    assert_eq!(page.messages[1].id, "a1");

    // A new chat has only its tail.
    let page = transcripts.page(None, tail, &Window::Latest(40)).unwrap();
    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.messages[0].id, "live-0");
}
