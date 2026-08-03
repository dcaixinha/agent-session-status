use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn malformed_alert_config_warns_after_successful_event_commit() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config/agent-session-status");
    let state_dir = dir.path().join("state");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.json"), "{not json").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-session-status"))
        .args([
            "--state-dir",
            state_dir.to_str().unwrap(),
            "event",
            "opencode",
        ])
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"type":"session.status","properties":{"sessionID":"one","status":{"type":"busy"}},"instanceDirectory":"/tmp/project"}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(state_dir.join("state.json").is_file());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning: idle alert failed"), "{stderr}");
    assert!(stderr.contains("config.json"), "{stderr}");
}

#[test]
fn alert_test_explains_disabled_defaults_without_touching_state() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_agent-session-status"))
        .args(["--state-dir", state_dir.to_str().unwrap(), "alert-test"])
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("idle alerts are disabled")
    );
    assert!(!state_dir.exists());
}
