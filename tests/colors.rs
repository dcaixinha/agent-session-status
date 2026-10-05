use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_agent-session-status");

fn seed_idle_session(state_dir: &Path, config_dir: &Path) {
    let mut snapshot = Command::new(BINARY)
        .args(["--state-dir", state_dir.to_str().unwrap(), "snapshot"])
        .env("XDG_CONFIG_HOME", config_dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    snapshot
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"source": "test-source", "ttl_seconds": 60, "instances": [{
                "id": "host-one", "label": "Test host",
                "sessions": [{"id": "s1", "provider": "opencode", "status": "idle", "cwd": "/tmp/p"}]
            }]}"#,
        )
        .unwrap();
    assert!(snapshot.wait().unwrap().success());
}

fn render(state_dir: &Path, config_dir: &Path, envs: &[(&str, &str)]) -> String {
    let mut command = Command::new(BINARY);
    command
        .args([
            "--state-dir",
            state_dir.to_str().unwrap(),
            "render",
            "--format",
            "ironbar",
            "--source",
            "test-source",
        ])
        .env("XDG_CONFIG_HOME", config_dir)
        .env_remove("AGENT_SESSION_STATUS_COLOR_IDLE")
        .env_remove("AGENT_SESSION_STATUS_COLOR_IDLE_LIGHT")
        .env_remove("AGENT_SESSION_STATUS_COLOR_IDLE_DARK");
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn status_colors_prefer_the_active_theme_variant() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let config_dir = temp.path().join("config");
    seed_idle_session(&state_dir, &config_dir);

    let themed = [
        ("AGENT_SESSION_STATUS_COLOR_IDLE", "#111111"),
        ("AGENT_SESSION_STATUS_COLOR_IDLE_LIGHT", "#222222"),
        ("AGENT_SESSION_STATUS_COLOR_IDLE_DARK", "#333333"),
    ];
    let dark = render(
        &state_dir,
        &config_dir,
        &[&themed[..], &[("AGENT_SESSION_STATUS_THEME", "dark")]].concat(),
    );
    assert!(dark.contains("#333333"), "{dark}");
    let light = render(
        &state_dir,
        &config_dir,
        &[&themed[..], &[("AGENT_SESSION_STATUS_THEME", "light")]].concat(),
    );
    assert!(light.contains("#222222"), "{light}");

    // Without a variant for the active theme, the plain variable applies.
    let fallback = render(
        &state_dir,
        &config_dir,
        &[
            ("AGENT_SESSION_STATUS_COLOR_IDLE", "#111111"),
            ("AGENT_SESSION_STATUS_COLOR_IDLE_LIGHT", "#222222"),
            ("AGENT_SESSION_STATUS_THEME", "dark"),
        ],
    );
    assert!(fallback.contains("#111111"), "{fallback}");
}

#[test]
fn watch_redraws_when_the_stylesheet_is_repointed() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let config_dir = temp.path().join("config");
    let ironbar = config_dir.join("ironbar");
    fs::create_dir_all(&ironbar).unwrap();
    fs::write(ironbar.join("style_light.css"), "").unwrap();
    fs::write(ironbar.join("style_dark.css"), "").unwrap();
    let stylesheet = ironbar.join("style.css");
    symlink("style_light.css", &stylesheet).unwrap();
    seed_idle_session(&state_dir, &config_dir);

    let mut watch = Command::new(BINARY)
        .args([
            "--state-dir",
            state_dir.to_str().unwrap(),
            "watch",
            "--format",
            "ironbar",
            "--source",
            "test-source",
        ])
        .env("XDG_CONFIG_HOME", &config_dir)
        .env_remove("IRONBAR_CSS")
        .env_remove("AGENT_SESSION_STATUS_THEME")
        .env("AGENT_SESSION_STATUS_COLOR_IDLE_LIGHT", "#222222")
        .env("AGENT_SESSION_STATUS_COLOR_IDLE_DARK", "#333333")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = watch.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let initial = receiver
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert!(initial.contains("#222222"), "{initial}");

    // Repoint the way `ln -sfn` does: a new link renamed over the old one.
    let tmp = ironbar.join("style.css.tmp");
    symlink("style_dark.css", &tmp).unwrap();
    fs::rename(&tmp, &stylesheet).unwrap();

    let redrawn = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("watch did not redraw after the theme switch")
        .unwrap();
    assert!(redrawn.contains("#333333"), "{redrawn}");

    watch.kill().unwrap();
    watch.wait().unwrap();
}
