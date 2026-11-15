use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::time::Duration;

use ludolph_engine::pi::{Options, Pi, Session};
use serde_json::json;

const GRACE: Duration = Duration::from_secs(5);

fn options(cwd: &Path, env: &[(&str, &str)]) -> Options {
    Options {
        program: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-pi.py"),
        cwd: cwd.to_owned(),
        session: Session::New("s1".into()),
        extensions: Vec::new(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn exists(pid: u64) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

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

    let (pi, mut events) = Pi::start(&options(dir.path(), &[])).unwrap();
    let tool = pi.call(json!({"type": "spawn_child"})).await.unwrap();
    let tool = tool["pid"].as_u64().unwrap();
    let exited = pi.call(json!({"type": "exit"})).await.unwrap_err();
    assert_eq!(exited.to_string(), "pi exited before answering exit");
    assert_eq!(events.recv().await, None);
    assert_eq!(pi.wait().await.unwrap().code(), Some(3));
    assert!(pi.call(json!({"type": "get_state"})).await.is_err());
    assert!(gone(tool).await, "pi's tool {tool} outlived its crash");
}
