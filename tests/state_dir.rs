use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::process::Command;

fn render() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-session-status"));
    command
        .args(["render", "--format", "details"])
        .env_remove("AGENT_SESSION_STATUS_STATE_DIR");
    command
}

#[test]
fn default_state_directory_is_private_and_uses_xdg_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = temp.path().join("runtime");
    let cache = temp.path().join("cache");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&runtime)
        .unwrap();

    let output = render()
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .unwrap();
    assert!(output.status.success());
    let runtime_state = runtime.join("agent-session-status");
    assert!(runtime_state.join("state.lock").is_file());
    assert!(!cache.join("agent-session-status").exists());
    assert_eq!(
        fs::metadata(&runtime_state).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let fallback_cache = temp.path().join("fallback-cache");
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let output = render()
        .env("XDG_RUNTIME_DIR", "")
        .env("XDG_CACHE_HOME", &fallback_cache)
        .output()
        .unwrap();
    assert!(output.status.success());
    let cached_state = fallback_cache.join(format!("agent-session-status-{}", boot_id.trim()));
    assert!(cached_state.join("state.lock").is_file());
    assert_eq!(
        fs::metadata(cached_state).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn explicit_state_directory_bypasses_default_xdg_paths() {
    let temp = tempfile::tempdir().unwrap();
    let explicit = temp.path().join("explicit");
    let output = render()
        .args(["--state-dir", explicit.to_str().unwrap()])
        .env("XDG_RUNTIME_DIR", "relative-runtime")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(explicit.join("state.lock").is_file());
}

#[test]
fn relative_runtime_directory_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let output = render()
        .current_dir(temp.path())
        .env("XDG_RUNTIME_DIR", "relative-runtime")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("XDG_RUNTIME_DIR must be an absolute path")
    );
    assert!(!temp.path().join("relative-runtime").exists());
    assert!(!temp.path().join("cache/agent-session-status").exists());
}
