use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ludolph_engine::engine::{AlertKind, ChatInfo, Config, Delivery, Engine, Status};
use ludolph_engine::server;
use ludolph_engine::settings::{Notify, Settings};
use ludolph_engine::transcript::{Part, Role, ToolState, Window};
use ludolph_relay_proto::LinkCode;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};

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
        home: dir.to_owned(),
        app: Some(dir.join("Ludolph.app")),
    };
    Engine::start(config)
        .await
        .unwrap_or_else(|e| panic!("{e:?}"))
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
    let view = engine.view(&id, Window::Latest(40)).await.unwrap();
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
                    Part::Tool {
                        id: "c1".into(),
                        name: "bash".into(),
                        subject: Some("ls".into()),
                        state: ToolState::Done,
                    },
                    text("Done.")
                ]
            ),
        ]
    );
    let output = engine.tool_output(&id, "c1").await.unwrap();
    assert_eq!(
        (output.text.as_str(), output.args["command"].as_str()),
        ("a.txt", Some("ls"))
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

    // A draft left empty is gone once its idle pi stops.
    let empty = engine.create(&project).await.unwrap();
    assert!(engine.chats().iter().any(|c| c.id == empty));
    until(&engine, |c| !c.iter().any(|c| c.id == empty)).await;
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
    assert_eq!(
        engine
            .view("s1", Window::Latest(40))
            .await
            .unwrap()
            .page
            .total,
        2
    );

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

/// Call `tool` as the phone does.
async fn tool(engine: &Engine, tool: &str, args: Value) -> anyhow::Result<Value> {
    ludolph_engine::tools::call(engine, tool.parse().unwrap(), args).await
}

/// The text of the chat's last message, once it's settled.
async fn last_reply(engine: &Engine, id: &str) -> String {
    until(engine, |c| c[0].status == Status::Idle).await;
    let view = engine.view(id, Window::Latest(40)).await.unwrap();
    let parts = &view.page.messages.last().unwrap().parts;
    match parts.last().unwrap() {
        Part::Text { text } => text.clone(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_extensions_dialog_waits_for_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let id = engine.create(dir.path()).await.unwrap();

    engine.send(&id, "ask", Vec::new()).await.unwrap();
    until(&engine, |c| c[0].status == Status::AwaitingInput).await;
    let view = tool(&engine, "chat_view", json!({"chat": id}))
        .await
        .unwrap();
    assert_eq!(view["chat"]["status"], "awaitingInput");
    assert_eq!(
        view["read"]["pendingInput"],
        json!({"id": "d1", "dialog": "select", "title": "Pick one", "message": null,
               "options": ["red", "blue"], "prefill": null})
    );
    let wrong = json!({"chat": id, "id": "d1", "value": "green"});
    let refused = tool(&engine, "respond_to_input", wrong).await.unwrap_err();
    assert!(refused.to_string().contains("options"), "{refused:#}");
    let answer = json!({"chat": id, "id": "d1", "value": "blue"});
    tool(&engine, "respond_to_input", answer.clone())
        .await
        .unwrap();
    assert_eq!(last_reply(&engine, &id).await, "You picked blue.");
    let again = tool(&engine, "respond_to_input", answer).await.unwrap_err();
    assert!(
        again.to_string().contains("no such open question"),
        "{again:#}"
    );

    // Interrupting dismisses the dialog.
    engine.send(&id, "ask", Vec::new()).await.unwrap();
    until(&engine, |c| c[0].status == Status::AwaitingInput).await;
    engine.interrupt(&id).await.unwrap();
    assert_eq!(last_reply(&engine, &id).await, "You picked cancelled.");

    // A dialog's own timeout ends the question.
    engine.send(&id, "ask-briefly", Vec::new()).await.unwrap();
    until(&engine, |c| c[0].status == Status::AwaitingInput).await;
    assert_eq!(last_reply(&engine, &id).await, "You picked nothing.");
    assert_eq!(engine.question(&id), None);
    engine.shutdown().await.unwrap();
}

/// The ids `list_chats` returns with `args`, newest first.
async fn listed(engine: &Engine, args: Value) -> Vec<String> {
    let chats = tool(engine, "list_chats", args).await.unwrap();
    ids(&chats["chats"])
}

fn ids(chats: &Value) -> Vec<String> {
    let chats = chats.as_array().unwrap().iter();
    chats
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Each group `list_chat_groups` returns with `args`: its project's id,
/// total, and chat ids.
async fn groups(engine: &Engine, args: Value) -> Vec<(Value, u64, Vec<String>)> {
    let groups = tool(engine, "list_chat_groups", args).await.unwrap();
    let groups = groups["groups"].as_array().unwrap().iter();
    groups
        .map(|g| {
            (
                g["project"]["id"].clone(),
                g["total"].as_u64().unwrap(),
                ids(&g["chats"]),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn chats_list_by_project_and_stay_archived_until_used_again() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("sessions/--app--");
    std::fs::create_dir_all(&folder).unwrap();
    let user = |at: i64| {
        let message = json!({"role": "user", "content": "hi", "timestamp": at});
        json!({"type": "message", "id": format!("m{at}"), "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z", "message": message})
    };
    let home = dir.path().display().to_string();
    for (id, at, cwd) in [
        ("a", 1_000, "/work/app"),
        ("b", 2_000, "/work/app"),
        ("c", 3_000, "/work/app"),
        ("d", 4_000, home.as_str()),
    ] {
        let header = json!({"type": "session", "version": 3, "id": id, "timestamp": "2026-05-01T10:00:00.000Z", "cwd": cwd});
        std::fs::write(
            folder.join(format!("{id}.jsonl")),
            format!("{header}\n{}\n", user(at)),
        )
        .unwrap();
    }
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let app = json!("/work/app");
    assert_eq!(
        groups(&engine, json!({"limit": 2})).await,
        [
            (Value::Null, 1, vec!["d".into()]),
            (app.clone(), 3, vec!["c".into(), "b".into()])
        ]
    );
    let more = json!({"limit": 2, "more": [{"project": "/work/app", "limit": 12}]});
    assert_eq!(groups(&engine, more).await[1].2, ["c", "b", "a"]);

    tool(&engine, "archive_chat", json!({"chat": "b"}))
        .await
        .unwrap();
    tool(&engine, "archive_chat", json!({"chat": "a"}))
        .await
        .unwrap();
    assert_eq!(listed(&engine, json!({})).await, ["d", "c"]);
    assert_eq!(listed(&engine, json!({"archived": true})).await, ["b", "a"]);
    assert_eq!(groups(&engine, json!({})).await[1].1, 1);
    tool(
        &engine,
        "archive_chat",
        json!({"chat": "a", "archived": false}),
    )
    .await
    .unwrap();
    assert_eq!(listed(&engine, json!({})).await, ["d", "c", "a"]);

    // A chat used again comes back, and the rest stay archived after a restart.
    tool(&engine, "archive_chat", json!({"chat": "c"}))
        .await
        .unwrap();
    append(&folder.join("b.jsonl"), &user(5_000));
    until(&engine, |c| c.iter().any(|c| c.id == "b" && !c.archived)).await;
    engine.shutdown().await.unwrap();
    drop(engine);
    let engine = self::engine(dir.path(), Duration::from_secs(60)).await;
    assert_eq!(listed(&engine, json!({})).await, ["b", "d", "a"]);
    assert_eq!(listed(&engine, json!({"archived": true})).await, ["c"]);
    engine.shutdown().await.unwrap();
}

/// Run git in `repo`, and return what it printed.
fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The `turnChanges` of the chat's view, once it has `runs` saved runs.
async fn turn_changes(engine: &Engine, id: &str, runs: usize) -> (Vec<Value>, Vec<Value>) {
    for _ in 0..250 {
        // A run's store goes before its diff is saved, so both are waited for,
        // before the view: one taken earlier may miss a run saved since.
        let saved = engine.run_counts(id).await.unwrap().len() == runs;
        let stored = saved && std::fs::read_dir(state_runs(engine)).map_or(0, Iterator::count) == 0;
        let view = tool(engine, "chat_view", json!({"chat": id}))
            .await
            .unwrap();
        let read = &view["read"];
        let messages = read["messages"].as_array().unwrap().clone();
        let settled = view["chat"]["status"] == "idle";
        let changes = read["turnChanges"].as_array().unwrap().clone();
        let users = messages.iter().filter(|m| m["role"] == "user").count();
        if stored && settled && users == runs && !changes.is_empty() {
            return (changes, messages);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the run's changes never showed");
}

/// Where runs keep their snapshots until their changes are saved.
fn state_runs(engine: &Engine) -> PathBuf {
    engine.config().state_dir.join("turns/runs")
}

#[tokio::test(flavor = "multi_thread")]
async fn each_run_keeps_what_it_changed_without_touching_the_repo() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    std::fs::write(repo.join("b.txt"), "one\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "secret.env\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "start"]);
    // A change you staged yourself, which must stay staged.
    std::fs::write(repo.join("b.txt"), "two\n").unwrap();
    git(&repo, &["add", "b.txt"]);
    let status = git(&repo, &["status", "--porcelain"]);
    let objects = git(&repo, &["count-objects"]);

    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let id = engine.create(&repo).await.unwrap();
    engine.send(&id, "edit", Vec::new()).await.unwrap();
    let (changes, messages) = turn_changes(&engine, &id, 1).await;
    assert_eq!(changes.len(), 1);
    let run = &changes[0];
    assert_eq!(
        (&run["files"], &run["additions"], &run["deletions"]),
        (&json!(2), &json!(2), &json!(0))
    );
    assert_eq!(run["after"], messages.last().unwrap()["id"]);

    let diff = tool(
        &engine,
        "turn_changes",
        json!({"chat": id, "turn": run["turn"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        diff["files"],
        json!([{"path": "a.txt"}, {"path": "new.txt"}])
    );
    let patch = diff["patch"].as_str().unwrap();
    assert!(
        patch.contains("+changed") && !patch.contains("secret"),
        "{patch}"
    );

    // A run that changes nothing gets no button, and ends the one before.
    engine.send(&id, "hi", Vec::new()).await.unwrap();
    let (changes, messages) = turn_changes(&engine, &id, 2).await;
    let second = messages.iter().rposition(|m| m["role"] == "user").unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["after"], messages[second - 1]["id"]);

    assert_eq!(status, "M  b.txt\n");
    let status = git(&repo, &["status", "--porcelain"]);
    assert_eq!(
        status, " M a.txt\nM  b.txt\n?? new.txt\n",
        "your staged change stays staged"
    );
    assert_eq!(
        git(&repo, &["count-objects"]),
        objects,
        "the repo got no objects"
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn chats_are_titled_as_soon_as_they_start() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("sessions/--old--/old.jsonl");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    let header = json!({"type": "session", "version": 3, "id": "old", "timestamp": "2026-05-01T10:00:00.000Z", "cwd": dir.path()});
    let message = json!({"role": "user", "content": "rename the old module", "timestamp": 1_000});
    let first = json!({"type": "message", "id": "m1", "parentId": null, "timestamp": "2026-05-01T10:00:00.000Z", "message": message});
    std::fs::write(&old, format!("{header}\n{first}\n")).unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let title = |chat: &ChatInfo| chat.title.clone();
    let saved = |chat: &ChatInfo| chat.summary.as_ref().and_then(|s| s.title.clone());

    let id = engine.create(dir.path()).await.unwrap();
    engine
        .send(&id, "fix the build please", Vec::new())
        .await
        .unwrap();
    let chats = until(&engine, |c| c.iter().any(|c| title(c).is_some())).await;
    let chat = chats.iter().find(|c| c.id == id).unwrap();
    assert_eq!(title(chat).as_deref(), Some("Fix The Build"));
    // pi wrote it to the session, so pi's own `/resume` shows it too.
    until(&engine, |c| {
        c.iter()
            .any(|c| saved(c).as_deref() == Some("Fix The Build"))
    })
    .await;
    engine.send(&id, "and the tests", Vec::new()).await.unwrap();

    // An old chat without a title gets one from its first prompt.
    engine.send("old", "more", Vec::new()).await.unwrap();
    let old = |c: &[ChatInfo]| c.iter().any(|c| c.id == "old" && saved(c).is_some());
    let chats = until(&engine, old).await;
    let chat = chats.iter().find(|c| c.id == "old").unwrap();
    assert_eq!(saved(chat).as_deref(), Some("Rename The Old"));
    let asked = std::fs::read_to_string(dir.path().join("pi.log-titles")).unwrap();
    assert_eq!(asked, "fix the build\nrename the old\n");

    // Dictation goes through the same sidecar model, with thinking off.
    let heard = json!({"text": " uh fix the um build "});
    let polished = tool(&engine, "polish_dictation", heard.clone())
        .await
        .unwrap();
    assert_eq!(polished, json!({"text": "Fix the build. (default, off)"}));
    let blank = tool(&engine, "polish_dictation", json!({"text": " "})).await;
    assert!(blank.is_err());

    // Picking a sidecar model takes effect at once, and lasts.
    let unknown = json!({"sidecarModel": "fake/none"});
    let refused = tool(&engine, "set_settings", unknown).await.unwrap_err();
    assert!(
        format!("{refused:#}").contains("no model fake/none"),
        "{refused:#}"
    );
    let picked = json!({"sidecarModel": "fake/quick", "notify": "always", "openAtLogin": false});
    let saved = tool(&engine, "set_settings", picked.clone()).await.unwrap();
    assert_eq!(saved, picked);
    let polished = tool(&engine, "polish_dictation", heard).await.unwrap();
    assert_eq!(
        polished,
        json!({"text": "Fix the build. (fake/quick, off)"})
    );
    assert_eq!(
        tool(&engine, "get_settings", json!({})).await.unwrap(),
        picked
    );
    let kept = Settings::load(&engine.config().state_dir).unwrap();
    assert_eq!(
        (kept.sidecar_model.as_deref(), kept.notify),
        (Some("fake/quick"), Notify::Always)
    );

    // Nothing opens at login until it's turned on, and turning it off undoes it.
    let item = ludolph_engine::login::path(&engine.config().home);
    assert!(!item.exists());
    let on = json!({"sidecarModel": "fake/quick", "notify": "always", "openAtLogin": true});
    tool(&engine, "set_settings", on).await.unwrap();
    let written = std::fs::read_to_string(&item).unwrap();
    assert!(
        written.contains("Ludolph.app") && written.contains("--hidden"),
        "{written}"
    );
    tool(&engine, "set_settings", picked).await.unwrap();
    assert!(!item.exists());
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn chats_use_the_model_and_thinking_level_picked_for_them() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let first = engine(dir.path(), Duration::from_secs(60)).await;
    let made = tool(
        &first,
        "create_chat",
        json!({"project": repo, "prompt": "hi", "model": "fake/deep", "reasoning": "high"}),
    )
    .await
    .unwrap();
    let id = made["chatId"].as_str().unwrap().to_owned();
    until(&first, |c| {
        c[0].status == Status::Idle && last_prompt(&c[0]) == Some("hi")
    })
    .await;
    let info = json!({"checkout": "local", "branch": "main", "model": "fake/deep",
                      "reasoning": "high", "contextUsage": {"tokens": 1500, "window": 200_000}});
    let composer = |engine| tool(engine, "composer_info", json!({"chat": id}));
    assert_eq!(composer(&first).await.unwrap(), info, "as its pi says");
    first.shutdown().await.unwrap();
    // Without a pi, the session file says the same, and asking doesn't
    // start one.
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    assert_eq!(composer(&engine).await.unwrap(), info, "as its file says");
    all_exited(dir.path()).await;

    let models = tool(&engine, "list_models", json!({})).await.unwrap();
    assert_eq!(
        models["models"],
        json!([
            {"id": "fake/deep", "label": "Deep", "reasoningLevels": ["off", "low", "medium", "high", "xhigh"]},
            {"id": "fake/quick", "label": "Quick", "reasoningLevels": ["off"]},
        ])
    );

    let quick = json!({"chat": id, "model": "fake/quick"});
    let saved = tool(&engine, "set_chat_model", quick).await.unwrap();
    assert_eq!(
        saved,
        json!({"model": "fake/quick", "reasoning": "off", "contextUsage": {"tokens": null, "window": 100_000}}),
        "the level fits the model"
    );
    let unknown = json!({"chat": id, "model": "nope/x"});
    let refused = tool(&engine, "set_chat_model", unknown).await.unwrap_err();
    assert!(
        format!("{refused:#}").contains("Model not found"),
        "{refused:#}"
    );

    let draft = tool(&engine, "composer_info", json!({"project": dir.path()}))
        .await
        .unwrap();
    assert_eq!(draft, json!({"checkout": null, "branch": null}));

    // A rename shows at once, and pi writes it to the file.
    let rename = json!({"chat": id, "title": " Fix the build "});
    tool(&engine, "rename_chat", rename).await.unwrap();
    assert_eq!(engine.chats()[0].title.as_deref(), Some("Fix the build"));
    let file = dir.path().join(format!("sessions/--fake--/{id}.jsonl"));
    let named = std::fs::read_to_string(file).unwrap();
    assert!(named.contains(r#""name": "Fix the build""#), "{named}");
    let empty = json!({"chat": id, "title": " "});
    assert!(tool(&engine, "rename_chat", empty).await.is_err());
    let log = std::fs::read_to_string(dir.path().join("pi.log")).unwrap();
    let listing = log.lines().filter(|l| l.ends_with("--no-session")).count();
    assert_eq!(listing, 1, "the models are listed once");
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_can_carry_images() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let png = json!({"mimeType": "image/png", "data": "iVBORw0KGgo="});
    let made = tool(
        &engine,
        "create_chat",
        json!({"project": dir.path(), "images": [png]}),
    )
    .await
    .unwrap();
    assert_eq!(made["delivery"], "run", "{made}");
    let id = made["chatId"].as_str().unwrap().to_owned();
    until(&engine, |c| {
        c[0].status == Status::Idle && c[0].summary.is_some()
    })
    .await;
    let view = tool(&engine, "chat_view", json!({"chat": id}))
        .await
        .unwrap();
    let first = &view["read"]["messages"][0];
    assert_eq!(
        first["parts"],
        json!([{"kind": "image", "index": 0}]),
        "{first}"
    );
    let image = json!({"chat": id, "message": first["id"], "index": 0});
    assert_eq!(tool(&engine, "chat_image", image).await.unwrap(), png);
    // An image alone gives a title nothing to go on.
    assert_eq!(engine.chats()[0].title, None);

    let tiff = json!({"chat": id, "text": "and this", "images": [{"mimeType": "image/tiff", "data": "AA=="}]});
    let refused = tool(&engine, "send_message", tiff).await.unwrap_err();
    assert!(refused.to_string().contains("image/tiff"), "{refused:#}");
    let empty = json!({"chat": id, "text": " "});
    assert!(tool(&engine, "send_message", empty).await.is_err());
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skill_runs_from_wherever_it_was_typed() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;
    let review =
        json!([{"name": "review", "description": "Review the diff", "ref": "/skill:review"}]);
    let skills = tool(&engine, "list_skills", json!({"project": dir.path()}))
        .await
        .unwrap();
    assert_eq!(skills["skills"], review, "for a new chat");

    let id = engine.create(dir.path()).await.unwrap();
    let skills = tool(&engine, "list_skills", json!({"chat": id}))
        .await
        .unwrap();
    assert_eq!(skills["skills"], review, "for a chat");
    // The composer puts the ref where `$review` was typed.
    let text = "please /skill:review\nthe last commit";
    tool(&engine, "send_message", json!({"chat": id, "text": text}))
        .await
        .unwrap();
    let typed = "/skill:review please\nthe last commit";
    until(&engine, |c| {
        last_prompt(&c[0]) == Some("/skill:review please the last commit")
    })
    .await;
    let view = tool(&engine, "chat_view", json!({"chat": id}))
        .await
        .unwrap();
    let user = &view["read"]["messages"][0];
    assert_eq!(
        user["parts"],
        json!([{"kind": "text", "text": typed}]),
        "pi ran it, and it shows as typed"
    );
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

    // A run cut short by a restart shows so after it, with no notice, since
    // the engine was stopped on purpose.
    engine.send(&id, "ask", Vec::new()).await.unwrap();
    until(&engine, |c| c[0].status == Status::AwaitingInput).await;
    let mut alerts = engine.alerts();
    engine.shutdown().await.unwrap();
    while let Ok(alert) = alerts.try_recv() {
        assert_ne!(alert.kind, AlertKind::Interrupted);
    }
    drop(engine);
    let restarted = self::engine(dir.path(), Duration::from_secs(60)).await;
    until(&restarted, |c| c[0].status == Status::Interrupted).await;
    restarted.shutdown().await.unwrap();
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
    let notice = seen.iter().find(|r| r["event"] == "chats").unwrap();
    assert_eq!(notice["subscription"], 1, "{notice}");
    assert!(seen.contains(&json!({"id": 3, "error": "no tool nope"})));

    // A line past the limit is refused, and the connection ends.
    let (huge_read, mut huge_write) = UnixStream::connect(&socket).await.unwrap().into_split();
    let huge = vec![b'x'; usize::try_from(server::MAX_LINE).unwrap() + 1];
    let writing = tokio::spawn(async move { huge_write.write_all(&huge).await });
    let mut huge_read = BufReader::new(huge_read);
    let mut refused = String::new();
    huge_read.read_line(&mut refused).await.unwrap();
    assert!(refused.contains("a request longer than"), "{refused}");
    refused.clear();
    assert_eq!(huge_read.read_line(&mut refused).await.unwrap(), 0);
    let _ = writing.await;
    let created = seen.iter().find(|r| r["id"] == 2).unwrap().clone();
    assert_eq!(created["ok"]["delivery"], "run", "{created}");
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

    // A chat's subscriber gets its page, then what changes as it runs.
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
    let mut frames: Vec<Value> = seen.into_iter().filter(|r| r["event"] == "chat").collect();
    assert_eq!(frames[0]["reset"], true, "{}", frames[0]);
    assert_eq!(frames[0]["subscription"], 6, "{}", frames[0]);
    // Once pi is gone after the run, nothing changes the view.
    until(&engine, |c| last_prompt(&c[0]) == Some("more")).await;
    all_exited(dir.path()).await;
    let request = json!({"id": 8, "call": "chat_view", "args": {"chat": chat, "limit": 3}});
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let seen = read_until(&mut reader, |seen| seen.iter().any(|r| r["id"] == 8)).await;
    frames.extend(seen.iter().filter(|r| r["event"] == "chat").cloned());
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
    frames.extend(seen.into_iter().filter(|r| r["event"] == "chat"));

    // Played back, the frames show what chat_view reads, though most of them
    // left out the messages that hadn't changed.
    let page = tool(&engine, "chat_view", json!({"chat": chat}))
        .await
        .unwrap();
    let page = page["read"]["messages"].as_array().unwrap().clone();
    let seen = read_until(&mut reader, |seen| {
        replay(frames.iter().chain(seen)) == page
    })
    .await;
    frames.extend(seen);
    assert!(
        frames.iter().any(|f| f["reset"] == false
            && f["messages"].as_array().unwrap().len() < f["ids"].as_array().unwrap().len()),
        "{frames:#?}"
    );
    assert!(
        frames
            .iter()
            .all(|f| f["chatId"] == *chat && f["error"].is_null())
    );
    let last = frames.last().unwrap();
    assert_eq!(
        (&last["total"], &last["olderRemaining"]),
        (&json!(4), &json!(0)),
        "{last}"
    );

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

/// The messages a subscriber to a chat's feed shows after `frames`.
fn replay<'a>(frames: impl IntoIterator<Item = &'a Value>) -> Vec<Value> {
    let mut have = std::collections::HashMap::new();
    let mut ids = Vec::new();
    for frame in frames.into_iter().filter(|r| r["event"] == "chat") {
        if frame["reset"] == true {
            have.clear();
        }
        for message in frame["messages"].as_array().unwrap() {
            have.insert(message["id"].as_str().unwrap().to_owned(), message.clone());
        }
        ids = frame["ids"].as_array().unwrap().clone();
    }
    ids.iter()
        .map(|id| have[id.as_str().unwrap()].clone())
        .collect()
}

/// Keeps the `Authorization` header of the Mac's handshake.
struct Bearer<'a>(&'a mut Option<String>);

impl Callback for Bearer<'_> {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let header = request.headers().get("authorization");
        *self.0 = header
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(response)
    }
}

/// The next message the Mac sent the relay.
async fn from_mac(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
    loop {
        if let Message::Text(body) = socket.next().await.unwrap().unwrap() {
            return serde_json::from_str(&body).unwrap();
        }
    }
}

/// The first message from the Mac that `wanted` picks; it skips the rest.
async fn until_from_mac(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    wanted: impl Fn(&Value) -> bool,
) -> Value {
    let wait = async {
        loop {
            let message = from_mac(socket).await;
            if wanted(&message) {
                return message;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_relay_link_answers_the_phones_calls_and_tells_it_what_happened() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), Duration::from_secs(60)).await;

    // A relay that refuses the token.
    let refusing = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = format!("http://{}", refusing.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut tcp, _) = refusing.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = tokio::io::AsyncReadExt::read(&mut tcp, &mut request).await;
        let refusal = b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n";
        tcp.write_all(refusal).await.unwrap();
    });
    let token = "t0ken".to_owned();
    let code = |relay: &str| {
        let code = LinkCode {
            relay: relay.into(),
            token: token.clone(),
        };
        json!({"code": code.encode()})
    };
    assert_eq!(
        tool(&engine, "get_phone_link", json!({})).await.unwrap(),
        Value::Null
    );
    let bad = tool(&engine, "link_phone", json!({"code": "nope"})).await;
    assert!(format!("{:#}", bad.unwrap_err()).contains("isn't a link code"));
    // Without TLS, the token only goes to a relay on this Mac.
    let plain = tool(&engine, "link_phone", code("http://relay.example.com")).await;
    let plain = format!("{:#}", plain.unwrap_err());
    assert!(plain.contains("https://"), "{plain}");
    let refused = tool(&engine, "link_phone", code(&relay)).await;
    let refused = format!("{:#}", refused.unwrap_err());
    assert!(refused.contains("rejected"), "{refused}");
    assert_eq!(
        tool(&engine, "get_phone_link", json!({})).await.unwrap(),
        Value::Null
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = format!("http://{}", listener.local_addr().unwrap());
    // Whether or not someone uses the Mac running the test.
    tool(&engine, "set_settings", json!({"notify": "always"}))
        .await
        .unwrap();
    let linking = tokio::spawn({
        let (engine, code) = (engine.clone(), code(&relay));
        async move { tool(&engine, "link_phone", code).await }
    });
    // Linking checks the token on a visit of its own, then connects to stay.
    let mut bearer = None;
    let (tcp, _) = listener.accept().await.unwrap();
    let mut visit = tokio_tungstenite::accept_hdr_async(tcp, Bearer(&mut bearer))
        .await
        .unwrap();
    assert_eq!(bearer.take().as_deref(), Some("Bearer t0ken"));
    assert_eq!(from_mac(&mut visit).await["type"], "hello");
    let (tcp, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_hdr_async(tcp, Bearer(&mut bearer))
        .await
        .unwrap();
    assert_eq!(bearer.as_deref(), Some("Bearer t0ken"));
    assert_eq!(linking.await.unwrap().unwrap()["relay"], relay);
    assert_eq!(from_mac(&mut socket).await["protocol"], 5);
    let linked = tool(&engine, "get_phone_link", json!({})).await.unwrap();
    assert_eq!(
        linked,
        json!({"relay": relay, "status": "connected", "error": null})
    );
    assert!(dir.path().join("state/relay.json").exists());
    // A Mac without a battery, like a CI machine, reports none.
    let power = from_mac(&mut socket).await;
    let battery = &power["battery"];
    assert_eq!(power["type"], "power");
    assert!(
        battery.is_null() || battery["percent"].as_u64().is_some_and(|p| p <= 100),
        "{power}"
    );

    let calls = [
        json!("nonsense"),
        json!({"type": "call", "id": 7, "tool": "list_chats", "args": {}}),
        json!({"type": "call", "id": 8, "tool": "open_terminal", "args": {}}),
        json!({"type": "call", "id": 6, "tool": "set_settings", "args": {"sidecarModel": null}}),
    ];
    for call in calls {
        let text = call
            .as_str()
            .map_or_else(|| call.to_string(), str::to_owned);
        socket.send(Message::Text(text.into())).await.unwrap();
    }
    let mut replies = std::collections::BTreeMap::new();
    while replies.len() < 3 {
        let reply = from_mac(&mut socket).await;
        replies.insert(reply["id"].as_u64().unwrap(), reply["result"].clone());
    }
    assert_eq!(replies[&7], json!({"Ok": {"total": 0, "chats": []}}));
    assert_eq!(replies[&8], json!({"Err": "no tool open_terminal"}));
    assert_eq!(
        replies[&6],
        json!({"Err": "the phone can't call set_settings"})
    );

    // Each moment worth telling reaches the phone as a notice.
    let create = json!({"type": "call", "id": 9, "tool": "create_chat", "args": {"project": dir.path(), "prompt": "ask"}});
    socket
        .send(Message::Text(create.to_string().into()))
        .await
        .unwrap();
    let made = until_from_mac(&mut socket, |m| m["id"] == 9).await;
    let chat = made["result"]["Ok"]["chatId"].as_str().unwrap().to_owned();
    let asked = until_from_mac(&mut socket, |m| m["type"] == "notify").await;
    assert_eq!(
        (&asked["kind"], &asked["chat"]),
        (&json!("input"), &json!(chat))
    );
    assert_eq!(asked["body"], "Needs your answer: Pick one");
    let answer = json!({"type": "call", "id": 10, "tool": "respond_to_input", "args": {"chat": chat,
                        "id": "d1", "value": "red"}});
    socket
        .send(Message::Text(answer.to_string().into()))
        .await
        .unwrap();
    let finished = until_from_mac(&mut socket, |m| m["type"] == "notify").await;
    assert_eq!(
        (&finished["kind"], &finished["body"]),
        (&json!("finished"), &json!("You picked red."))
    );
    // The phone follows a chat as frames of its feed, until it stops.
    for watch in [
        json!({"type": "watch", "id": 20, "chat": chat}),
        json!({"type": "watch", "id": 21, "chat": "nope"}),
    ] {
        socket
            .send(Message::Text(watch.to_string().into()))
            .await
            .unwrap();
    }
    // In any order: the missing chat's answer needs no reading.
    let mut frames = std::collections::BTreeMap::new();
    while frames.len() < 2 {
        let frame = until_from_mac(&mut socket, |m| m["type"] == "frame").await;
        frames
            .entry(frame["id"].as_u64().unwrap())
            .or_insert(frame["frame"].clone());
    }
    assert_eq!(frames[&20]["reset"], true, "{}", frames[&20]);
    assert_eq!(
        frames[&20]["ids"].as_array().unwrap().len(),
        2,
        "{}",
        frames[&20]
    );
    assert_eq!(frames[&21], json!({"error": "no chat nope"}));
    let unwatch = json!({"type": "unwatch", "id": 20});
    socket
        .send(Message::Text(unwatch.to_string().into()))
        .await
        .unwrap();

    let crash = json!({"type": "call", "id": 11, "tool": "send_message", "args": {"chat": chat, "text": "crash"}});
    socket
        .send(Message::Text(crash.to_string().into()))
        .await
        .unwrap();
    let cut = until_from_mac(&mut socket, |m| m["type"] == "notify").await;
    assert_eq!(
        (&cut["kind"], &cut["body"]),
        (&json!("interrupted"), &json!("The run stopped: pi exited."))
    );

    // Unlinking drops the connection and forgets the link.
    tool(&engine, "unlink_phone", json!({})).await.unwrap();
    let closed = async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), closed)
        .await
        .unwrap();
    assert_eq!(
        tool(&engine, "get_phone_link", json!({})).await.unwrap(),
        Value::Null
    );
    assert!(!dir.path().join("state/relay.json").exists());
    engine.shutdown().await.unwrap();
}
