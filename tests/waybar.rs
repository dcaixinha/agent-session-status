use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

#[test]
fn waybar_watch_emits_immediately_and_after_a_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let config_dir = temp.path().join("config");
    let binary = env!("CARGO_BIN_EXE_agent-session-status");

    let mut watch = Command::new(binary)
        .args([
            "--state-dir",
            state_dir.to_str().unwrap(),
            "watch",
            "--format",
            "waybar",
            "--source",
            "test-source",
            "--group-source",
        ])
        .env("XDG_CONFIG_HOME", &config_dir)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let initial = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("watch did not emit its initial line")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&initial).unwrap(),
        serde_json::json!({"text": "", "tooltip": "", "class": ["empty"]})
    );

    let mut snapshot = Command::new(binary)
        .args(["--state-dir", state_dir.to_str().unwrap(), "snapshot"])
        .env("XDG_CONFIG_HOME", &config_dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    snapshot
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{
                "source": "test-source",
                "ttl_seconds": 60,
                "instances": [{
                    "id": "host-one",
                    "label": "Test host",
                    "sessions": [{
                        "id": "session-one",
                        "provider": "opencode",
                        "status": "working",
                        "cwd": "/tmp/project"
                    }]
                }]
            }"#,
        )
        .unwrap();
    assert!(snapshot.wait().unwrap().success());

    let updated = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("watch did not emit after the snapshot")
        .unwrap();
    let updated: Value = serde_json::from_str(&updated).unwrap();
    assert!(updated["text"].as_str().unwrap().contains("test-source"));
    assert!(updated["tooltip"].as_str().unwrap().contains("Test host"));
    assert_eq!(updated["class"], serde_json::json!(["working"]));

    watch.kill().unwrap();
    watch.wait().unwrap();
    reader.join().unwrap();
}
