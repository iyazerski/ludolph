use std::io::{BufRead, BufReader};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ludolph_engine::runtime::Runtime;

/// A process in its own group, as the engine starts pi.
fn spawn(script: &str) -> Child {
    Command::new("sh")
        .args(["-c", script])
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn reap_stops_what_a_previous_engine_left_running() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let grace = Duration::from_millis(300);
    let (mut runtime, interrupted) = Runtime::reap(&path, grace).await.unwrap();
    assert!(interrupted.is_empty());

    let mut polite = spawn("exec sleep 60");
    // Ignores SIGTERM, and so does the tool it started.
    let mut stubborn = spawn("trap '' TERM; sleep 60 & echo $!; wait");
    let mut line = String::new();
    BufReader::new(stubborn.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let tool: u32 = line.trim().parse().unwrap();
    runtime.add("polite", polite.id()).unwrap();
    runtime.add("stubborn", stubborn.id()).unwrap();
    runtime.set_running("stubborn", true).unwrap();
    drop(runtime);

    let (_, interrupted) = Runtime::reap(&path, grace).await.unwrap();
    assert_eq!(interrupted, ["stubborn"]);
    assert_eq!(polite.wait().unwrap().signal(), Some(libc::SIGTERM));
    assert_eq!(stubborn.wait().unwrap().signal(), Some(libc::SIGKILL));
    let ps = Command::new("ps")
        .args(["-p", &tool.to_string()])
        .output()
        .unwrap();
    assert!(
        !ps.status.success(),
        "the stubborn process's tool {tool} survived"
    );
}

#[tokio::test]
async fn reap_leaves_a_reused_pid_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let mut other = spawn("exec sleep 60");
    // The pid is right, but the process started at another time.
    let saved = format!(
        r#"{{"s1": {{"pid": {}, "started": 1, "running": true}}}}"#,
        other.id()
    );
    std::fs::write(&path, saved).unwrap();

    let (_, interrupted) = Runtime::reap(&path, Duration::from_millis(100))
        .await
        .unwrap();
    assert_eq!(interrupted, ["s1"]);
    assert_eq!(other.try_wait().unwrap(), None);
    other.kill().unwrap();
    other.wait().unwrap();
}
