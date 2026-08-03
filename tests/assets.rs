use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn command(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-session-status"));
    command
        .args(["--state-dir", temp.path().join("state").to_str().unwrap()])
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_RUNTIME_DIR", temp.path().join("runtime"));
    command
}

#[test]
fn custom_asset_supports_theme_overrides_and_explicit_tinting() {
    let temp = tempfile::tempdir().unwrap();
    let generic = temp.path().join("generic.svg");
    let dark = temp.path().join("dark.svg");
    fs::write(&generic, "<svg><path fill='#123456'/></svg>").unwrap();
    fs::write(&dark, "<svg><path fill='#654321'/></svg>").unwrap();

    let output = command(&temp)
        .args(["asset", "claude"])
        .env("AGENT_SESSION_STATUS_THEME", "dark")
        .env("AGENT_SESSION_STATUS_ASSET_CLAUDE", &generic)
        .env("AGENT_SESSION_STATUS_ASSET_CLAUDE_DARK", &dark)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        dark.to_str().unwrap()
    );
    assert!(!temp.path().join("state").exists());

    let output = command(&temp)
        .args([
            "asset",
            "claude",
            "--status-color",
            "--source-color",
            "#654321",
        ])
        .env("AGENT_SESSION_STATUS_THEME", "dark")
        .env("AGENT_SESSION_STATUS_ASSET_CLAUDE_DARK", &dark)
        .env("AGENT_SESSION_STATUS_COLOR_IDLE", "#abcdef")
        .output()
        .unwrap();
    assert!(output.status.success());
    let tinted = String::from_utf8(output.stdout).unwrap();
    assert!(
        fs::read_to_string(tinted.trim())
            .unwrap()
            .contains("#abcdef")
    );
}

#[test]
fn asset_searches_system_xdg_data_and_rejects_incomplete_tint_options() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system/agent-session-status");
    fs::create_dir_all(&system).unwrap();
    let asset = system.join("claude-logo-light-square.svg");
    fs::write(&asset, "<svg/>").unwrap();

    let output = command(&temp)
        .args(["asset", "claude"])
        .env("AGENT_SESSION_STATUS_THEME", "light")
        .env("XDG_DATA_HOME", temp.path().join("user"))
        .env("XDG_DATA_DIRS", temp.path().join("system"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        asset.to_str().unwrap()
    );

    let output = command(&temp)
        .args(["asset", "claude", "--source-color", "#123456"])
        .env("AGENT_SESSION_STATUS_ASSET_CLAUDE", &asset)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--source-color and a tint option must be used together")
    );
}

#[test]
fn tint_cache_fallback_is_private_and_atomic_output_is_complete() {
    let temp = tempfile::tempdir().unwrap();
    let asset = temp.path().join("custom.svg");
    fs::write(&asset, "<svg><path fill='#123456'/></svg>").unwrap();

    let output = command(&temp)
        .args([
            "asset",
            "claude",
            "--foreground-color",
            "--source-color",
            "#123456",
        ])
        .env("AGENT_SESSION_STATUS_ASSET_CLAUDE", &asset)
        .env("AGENT_SESSION_STATUS_COLOR_FOREGROUND", "#abcdef")
        .env_remove("XDG_RUNTIME_DIR")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let tinted = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert_eq!(
        fs::read_to_string(&tinted).unwrap(),
        "<svg><path fill='#abcdef'/></svg>"
    );
    let application = tinted.parent().unwrap().parent().unwrap();
    assert_eq!(
        fs::metadata(application).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
