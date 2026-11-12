use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludolph_engine::engine::{ChatInfo, Config, Delivery, Engine, Status};
use ludolph_engine::server;
use ludolph_engine::transcript::{Part, Role};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// An engine in `dir` that runs the fake pi, which logs to `dir/pi.log`.
async fn engine(dir: &Path, idle: Duration) -> Engine {
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-pi.py");
    let sessions = dir.join("sessions");
    let wrapper = dir.join("pi");
    let script = format!(
        "#!/bin/sh\nFAKE_PI_SESSIONS='{}' FAKE_PI_LOG='{}' exec '{}' \"$@\"\n",
        sessions.display(),
        dir.join("pi.log").display(),
        fake.display()
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = Config {
        pi: wrapper,
        sessions_dir: sessions,
        state_dir: dir.join("state"),
        idle,
        grace: Duration::from_secs(2),
    };
    Engine::start(config).await.unwrap()
}

/// Wait until the chat list passes `check`.
async fn until(engine: &Engine, check: impl Fn(&[ChatInfo]) -> bool) -> Vec<ChatInfo> {
    let mut changes = engine.subscribe();
    let wait = async {
        loop {
            let chats = engine.chats();
            if check(&chats) {
                return chats;
            }
            changes.changed().await.unwrap();
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .expect("the chat list never got there")
}

/// Wait until pi's log has `lines`.
async fn log_until(dir: &Path, lines: usize) -> Vec<String> {
    for _ in 0..250 {
        let log = std::fs::read_to_string(dir.join("pi.log")).unwrap_or_default();
        let log: Vec<String> = log.lines().map(str::to_owned).collect();
        if log.len() >= lines {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("pi's log never got {lines} lines");
}

/// Wait until every pi that started has exited.
async fn all_exited(dir: &Path) {
    for _ in 0..250 {
        let log = std::fs::read_to_string(dir.join("pi.log")).unwrap_or_default();
        let starts = log.lines().filter(|l| l.starts_with("start")).count();
        if starts > 0 && starts == log.lines().filter(|l| *l == "exit").count() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("pi never exited");
}

fn last_prompt(chat: &ChatInfo) -> Option<&str> {
    chat.summary.as_ref()?.last_prompt.as_deref()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_runs_stops_when_idle_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("app");
    std::fs::create_dir(&project).unwrap();
    let engine = engine(dir.path(), Duration::from_millis(300)).await;

    let id = engine.create(&project).await.unwrap();
    let chats = engine.chats();
    assert_eq!((chats.len(), &chats[0].id), (1, &id));
    assert_eq!(
        (chats[0].summary.as_ref(), chats[0].status),
        (None, Status::Idle)
    );

    assert_eq!(
        engine.send(&id, "hi", Vec::new()).await.unwrap(),
        Delivery::Started
    );
    let chats = until(&engine, |c| {
        c[0].status == Status::Idle && last_prompt(&c[0]) == Some("hi")
    })
    .await;
    assert_eq!(chats.len(), 1, "the draft became the session file's row");
    let view = engine.view(&id, 40).await.unwrap();
    let shown: Vec<(Role, Vec<Part>)> = view
        .page
        .messages
        .into_iter()
        .map(|m| (m.role, m.parts))
        .collect();
    let text = |text: &str| Part::Text { text: text.into() };
    assert_eq!(
        shown,
        [
            (Role::User, vec![text("hi")]),
            (
                Role::Assistant,
                vec![
                    text("Hello!"),
                    Part::Tools {
                        tools: vec!["exec: ls".into()]
                    },
                    text("Done.")
                ]
            ),
        ]
    );

    // The idle pi stops, and the next message resumes the session in a new one.
    let log = log_until(dir.path(), 2).await;
    assert_eq!(log[0], format!("start --mode rpc --session-id {id}"));
    assert_eq!(log[1], "exit");
    engine.send(&id, "again", Vec::new()).await.unwrap();
    until(&engine, |c| last_prompt(&c[0]) == Some("again")).await;
    let log = log_until(dir.path(), 3).await;
    assert!(
        log[2].starts_with("start --mode rpc --session "),
        "{}",
        log[2]
    );
    engine.shutdown().await.unwrap();
}

/// Append `entry` to `path` the way pi does: open, write, close. macOS
/// reports a write when the file is closed.
fn append(path: &Path, entry: &Value) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    std::io::Write::write_all(&mut file, format!("{entry}\n").as_bytes()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn another_pis_writes_reach_the_chat_and_turn_sending_off() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let path = dir.path().join("sessions/--app--/s1.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let user = |id: &str, parent: Option<&str>, text: &str, at: i64| {
        let message = json!({"role": "user", "content": text, "timestamp": at});
        json!({"type": "message", "id": id, "parentId": parent, "timestamp": "2026-05-01T10:00:00.000Z", "message": message})
    };
    let header = json!({"type": "session", "version": 3, "id": "s1", "timestamp": "2026-05-01T10:00:00.000Z", "cwd": dir.path()});
    std::fs::write(
        &path,
        format!("{header}\n{}\n", user("a", None, "first", 1)),
    )
    .unwrap();
    let chats = until(&engine, |c| {
        c.len() == 1 && last_prompt(&c[0]) == Some("first")
    })
    .await;
    assert_eq!((chats[0].id.as_str(), chats[0].elsewhere), ("s1", true));

    let mut changes = engine.subscribe_chat("s1").unwrap();
    append(&path, &user("b", Some("a"), "second", 2));
    tokio::time::timeout(Duration::from_secs(5), changes.changed())
        .await
        .expect("the chat never changed")
        .unwrap();
    assert_eq!(engine.view("s1", 40).await.unwrap().page.total, 2);

    // Sending waits for a take-over, and the chat's own prompts don't count
    // as another pi's.
    let refused = engine.send("s1", "mine", Vec::new()).await.unwrap_err();
    assert!(refused.to_string().contains("another pi"), "{refused:#}");
    engine.take_over("s1").await.unwrap();
    engine.send("s1", "mine", Vec::new()).await.unwrap();
    until(&engine, |c| {
        c[0].status == Status::Idle && last_prompt(&c[0]) == Some("mine")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!engine.chats()[0].elsewhere);

    // Another pi's prompt while the chat's pi is open stops that pi.
    let leaf = engine.chats()[0].summary.as_ref().unwrap().leaf.clone();
    append(&path, &user("c", leaf.as_deref(), "theirs", 3));
    until(&engine, |c| c[0].elsewhere).await;
    all_exited(dir.path()).await;

    std::fs::remove_file(&path).unwrap();
    until(&engine, <[ChatInfo]>::is_empty).await;
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_that_dies_mid_run_leaves_the_chat_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let id = engine.create(dir.path()).await.unwrap();
    engine.send(&id, "crash", Vec::new()).await.unwrap();
    until(&engine, |c| c[0].status == Status::Interrupted).await;

    engine.send(&id, "retry", Vec::new()).await.unwrap();
    until(&engine, |c| {
        c[0].status == Status::Idle && last_prompt(&c[0]) == Some("retry")
    })
    .await;
    engine.shutdown().await.unwrap();
}

/// Read records from the socket until those read so far pass `check`.
async fn read_until(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    check: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    let mut seen = Vec::new();
    let read = async {
        while !check(&seen) {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            seen.push(serde_json::from_str(&line).unwrap());
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("no such record");
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_answers_calls_and_pushes_list_changes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_millis(200)).await;
    let socket = dir.path().join("state/engine.sock");
    let listener = server::bind(&socket).unwrap();
    assert!(
        server::bind(&socket).is_err(),
        "a second engine starts on the same socket"
    );
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    tokio::spawn(server::serve(engine.clone(), listener));

    let (read, mut write) = UnixStream::connect(&socket).await.unwrap().into_split();
    let mut reader = BufReader::new(read);
    let project: PathBuf = dir.path().into();
    let requests = [
        json!({"id": 1, "subscribe": "chats"}),
        json!({"id": 2, "call": "create_chat", "args": {"project": project, "prompt": "hi"}}),
        json!({"id": 3, "call": "nope"}),
    ];
    for request in requests {
        write
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }
    // Calls run at the same time, so their answers come in any order.
    let seen = read_until(&mut reader, |seen| {
        let has = |key: &str, value: Value| seen.iter().any(|r| r[key] == value);
        has("id", json!(2)) && has("id", json!(3)) && has("event", json!("chats"))
    })
    .await;
    assert!(seen.contains(&json!({"id": 1, "ok": null})));
    assert!(seen.contains(&json!({"id": 3, "error": "no tool nope"})));
    let created = seen.iter().find(|r| r["id"] == 2).unwrap().clone();
    assert_eq!(created["ok"]["sent"]["delivery"], "run", "{created}");
    assert_eq!(
        created["ok"]["project"]["name"],
        json!(dir.path().file_name().unwrap().to_str())
    );

    write
        .write_all(format!("{}\n", json!({"id": 4, "call": "list_chats"})).as_bytes())
        .await
        .unwrap();
    let seen = read_until(&mut reader, |seen| seen.iter().any(|r| r["id"] == 4)).await;
    let listed = &seen.iter().find(|r| r["id"] == 4).unwrap()["ok"];
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["chats"][0]["id"], created["ok"]["chatId"]);

    // A chat's subscriber hears of its run, and reads it with chat_view.
    let chat = &created["ok"]["chatId"];
    let requests = [
        json!({"id": 6, "subscribe": "chat", "args": {"chat": chat}}),
        json!({"id": 7, "call": "send_message", "args": {"chat": chat, "text": "more"}}),
    ];
    for request in requests {
        write
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }
    let seen = read_until(&mut reader, |seen| {
        seen.iter().any(|r| r["id"] == 7) && seen.iter().any(|r| r["event"] == "chat")
    })
    .await;
    assert!(seen.contains(&json!({"id": 6, "ok": null})));
    // Once pi is gone after the run, nothing changes the view.
    until(&engine, |c| last_prompt(&c[0]) == Some("more")).await;
    all_exited(dir.path()).await;
    let request = json!({"id": 8, "call": "chat_view", "args": {"chat": chat, "limit": 3}});
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let seen = read_until(&mut reader, |seen| seen.iter().any(|r| r["id"] == 8)).await;
    let view = &seen.iter().find(|r| r["id"] == 8).unwrap()["ok"];
    assert_eq!(view["read"]["total"], 4, "{view}");
    assert_eq!(view["read"]["olderRemaining"], 1, "{view}");
    let more = &view["read"]["messages"][1];
    assert_eq!(
        (&more["role"], &more["parts"]),
        (&json!("user"), &json!([{"kind": "text", "text": "more"}])),
        "{view}"
    );
    let request = json!({
        "id": 9, "call": "chat_view", "args": {"chat": chat, "limit": 3, "known": view["version"]},
    });
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let seen = read_until(&mut reader, |seen| seen.iter().any(|r| r["id"] == 9)).await;
    let unchanged = seen.iter().find(|r| r["id"] == 9).unwrap();
    assert_eq!(unchanged["ok"]["read"], Value::Null, "{unchanged}");

    // A client that closes its side after sending still gets the answer.
    let (read, mut write) = UnixStream::connect(&socket).await.unwrap().into_split();
    let request = json!({"id": 5, "call": "list_projects"});
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    write.shutdown().await.unwrap();
    let seen = read_until(&mut BufReader::new(read), |seen| !seen.is_empty()).await;
    assert_eq!(seen[0]["ok"]["projects"].as_array().unwrap().len(), 1);
    engine.shutdown().await.unwrap();
}
