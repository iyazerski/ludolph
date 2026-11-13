use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludolph_engine::live::Live;
use ludolph_engine::pi::{Event, Options, Pi, Session};
use serde_json::{Value, json};

const GRACE: Duration = Duration::from_secs(5);

fn options(cwd: &Path, env: &[(&str, &str)]) -> Options {
    Options {
        program: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-pi.py"),
        cwd: cwd.to_owned(),
        session: Session::New("s1".into()),
        extensions: vec![PathBuf::from("/ext/ask.ts")],
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

/// Whether process `pid` still exists.
fn exists(pid: u64) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

/// Wait up to a second for process `pid` to be gone, and return whether it is.
async fn gone(pid: u64) -> bool {
    for _ in 0..50 {
        if !exists(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn calls_and_events_reach_the_live_tail() {
    let dir = tempfile::tempdir().unwrap();
    let (pi, mut events) = Pi::start(&options(dir.path(), &[])).unwrap();

    let state = pi.call(json!({"type": "get_state"})).await.unwrap();
    let args = [
        "--mode",
        "rpc",
        "--session-id",
        "s1",
        "--extension",
        "/ext/ask.ts",
    ];
    assert_eq!(state["args"], json!(args));
    assert_eq!(
        Path::new(state["cwd"].as_str().unwrap()),
        dir.path().canonicalize().unwrap()
    );
    let refused = pi.call(json!({"type": "nope"})).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "pi refused nope: Unknown command: nope"
    );

    let started = pi
        .call(json!({"type": "prompt", "message": "hi"}))
        .await
        .unwrap();
    assert_eq!(started, json!({"disposition": "started"}));
    let mut live = Live::new();
    let mut streamed = Vec::new();
    let mut tool_progress = Vec::new();
    while let Some(event) = events.recv().await {
        live.apply(&event);
        if let Some(message) = live.streaming() {
            streamed.push(message["content"].clone());
        }
        if let Some(tool) = live.tools().get("c1") {
            tool_progress.push(tool["partialResult"].clone());
        }
        if matches!(event, Event::AgentSettled { .. }) {
            break;
        }
    }
    let call =
        json!({"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "ls"}});
    let partial_call = |arguments: &str| {
        json!({"type": "toolCall", "id": "c1", "name": "bash", "arguments": {},
               "partialArguments": arguments})
    };
    assert_eq!(
        streamed,
        [
            json!([]),
            json!([{"type": "text", "text": ""}]),
            json!([{"type": "text", "text": "Hel"}]),
            json!([{"type": "text", "text": "Hello"}]),
            // Each block's end replaces what its deltas built.
            json!([{"type": "text", "text": "Hello!"}]),
            json!([{"type": "text", "text": "Hello!"}, partial_call("")]),
            json!([{"type": "text", "text": "Hello!"}, partial_call("{\"command\":")]),
            json!([{"type": "text", "text": "Hello!"}, call]),
        ]
    );
    assert_eq!(tool_progress, [Value::Null, json!("a.txt")]);
    let roles: Vec<&Value> = live.messages().iter().map(|m| &m["role"]).collect();
    assert_eq!(roles, ["user", "assistant", "toolResult", "assistant"]);
    assert!(!live.working() && live.streaming().is_none() && live.tools().is_empty());

    assert!(pi.stop(GRACE).await.unwrap().success());
}

#[test]
fn the_live_tail_keeps_a_failure_and_drops_bulky_output() {
    let mut live = Live::new();
    let apply = |live: &mut Live, event: Value| {
        live.apply(&serde_json::from_value::<Event>(event).unwrap())
    };
    apply(&mut live, json!({"type": "agent_start"}));
    apply(
        &mut live,
        json!({"type": "message_end", "message": {"role": "user", "timestamp": 1,
        "content": [{"type": "text", "text": "see"}, {"type": "image", "data": "AAAA", "mimeType": "image/png"}]}}),
    );
    apply(
        &mut live,
        json!({"type": "message_end", "message": {"role": "toolResult", "timestamp": 2,
        "toolCallId": "c1", "content": [{"type": "text", "text": "lots of output"}]}}),
    );
    // An error pi doesn't retry fails the run as much as one it gave up on.
    apply(
        &mut live,
        json!({"type": "message_end", "message": {"role": "assistant", "timestamp": 3,
        "stopReason": "error", "errorMessage": "invalid api key", "content": []}}),
    );
    assert_eq!(live.failure(), Some("invalid api key"));
    assert_eq!(live.messages()[0]["content"][1], json!({"type": "image"}));
    assert_eq!(live.messages()[1]["content"], json!([]));
    assert_eq!(live.messages()[1]["toolCallId"], "c1");

    apply(
        &mut live,
        json!({"type": "auto_retry_start", "attempt": 1, "maxAttempts": 3, "errorMessage": "overloaded"}),
    );
    assert_eq!(live.failure(), None);
}

#[tokio::test]
async fn stop_kills_a_pi_that_wont_exit_with_its_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (pi, _events) = Pi::start(&options(dir.path(), &[("FAKE_PI_STUBBORN", "1")])).unwrap();
    let child = pi.call(json!({"type": "spawn_child"})).await.unwrap();
    let child = child["pid"].as_u64().unwrap();

    let status = pi.stop(Duration::from_millis(200)).await.unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    assert!(gone(child).await, "pi's tool {child} outlived it");
}

#[tokio::test]
async fn a_crashed_pi_takes_its_tools_but_a_stopped_one_leaves_its_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let (pi, _events) = Pi::start(&options(dir.path(), &[])).unwrap();
    let job = pi.call(json!({"type": "spawn_child"})).await.unwrap();
    let job = job["pid"].as_u64().unwrap();
    assert!(pi.stop(GRACE).await.unwrap().success());
    let survived = exists(job);
    std::process::Command::new("kill")
        .arg(job.to_string())
        .status()
        .unwrap();
    assert!(survived, "a clean exit stopped the job it left");

    let (pi, _events) = Pi::start(&options(dir.path(), &[])).unwrap();
    let tool = pi.call(json!({"type": "spawn_child"})).await.unwrap();
    let tool = tool["pid"].as_u64().unwrap();
    pi.call(json!({"type": "exit"})).await.unwrap_err();
    assert_eq!(pi.wait().await.unwrap().code(), Some(3));
    assert!(gone(tool).await, "pi's tool {tool} outlived its crash");
}

#[tokio::test]
async fn calls_fail_once_pi_exits() {
    let dir = tempfile::tempdir().unwrap();
    let (pi, mut events) = Pi::start(&options(dir.path(), &[])).unwrap();
    let exited = pi.call(json!({"type": "exit"})).await.unwrap_err();
    assert_eq!(exited.to_string(), "pi exited before answering exit");
    assert_eq!(events.recv().await, None);
    assert_eq!(pi.wait().await.unwrap().code(), Some(3));
    assert!(pi.call(json!({"type": "get_state"})).await.is_err());
}
