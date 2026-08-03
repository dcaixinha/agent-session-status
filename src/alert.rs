use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

use crate::assets::{data_dir, development_asset, find_data_file};
use crate::config::{AlertScope, Config, LoadedConfig, home_dir};
use crate::model::{Provider, Session, SessionKind};

const DND_TIMEOUT: Duration = Duration::from_secs(2);
const SOUND_FILENAME: &str = "agent-complete.wav";

pub trait CommandRunner {
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> Result<()>;
    fn capture(&self, command: &[String], timeout: Duration) -> Result<String>;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn capture(&self, command: &[String], timeout: Duration) -> Result<String> {
        let Some((program, args)) = command.split_first() else {
            bail!("DND command is empty");
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    bail!("DND command exited with {status}");
                }
                let mut output = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    stdout.read_to_string(&mut output)?;
                }
                return Ok(output);
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("DND command timed out after {} seconds", timeout.as_secs());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub fn deliver_loaded(sessions: &[Session], loaded: &LoadedConfig) -> Result<()> {
    let fallback_data_dir = data_dir();
    let installed_sound = find_data_file(SOUND_FILENAME);
    let installed_data_dir = installed_sound
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(&fallback_data_dir);
    deliver_with(
        sessions,
        &loaded.config,
        &loaded.directory,
        &SystemRunner,
        installed_data_dir,
        development_asset(SOUND_FILENAME).as_deref(),
    )
}

pub fn deliver_with(
    sessions: &[Session],
    config: &Config,
    config_dir: &Path,
    runner: &dyn CommandRunner,
    installed_data_dir: &Path,
    development_sound: Option<&Path>,
) -> Result<()> {
    let alerts = &config.idle_alerts;
    if !alerts.notification && !alerts.sound {
        return Ok(());
    }
    let sessions: Vec<_> = sessions
        .iter()
        .filter(|session| alerts.include_subagents || session.kind == SessionKind::Main)
        .filter(|session| match alerts.scope {
            AlertScope::All => true,
            AlertScope::Local => is_local(session),
            AlertScope::Remote => !is_local(session),
        })
        .collect();
    if sessions.is_empty() {
        return Ok(());
    }

    let mut errors = Vec::new();
    if alerts.notification {
        let (title, body) = notification_text(&sessions);
        let args = [
            "--app-name".into(),
            "agent-session-status".into(),
            "--urgency".into(),
            "normal".into(),
            "--icon".into(),
            "dialog-information".into(),
            title.into(),
            body.into(),
        ];
        if let Err(error) = runner.spawn(OsStr::new("notify-send"), &args) {
            errors.push(format!("desktop notification failed: {error}"));
        }
    }

    let dnd_suppresses = alerts.sound
        && alerts.respect_dnd
        && !alerts.dnd_command.is_empty()
        && runner
            .capture(&alerts.dnd_command, DND_TIMEOUT)
            .map(|output| {
                matches!(
                    output.trim().to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes" | "on"
                )
            })
            .unwrap_or(true);
    if alerts.sound && !dnd_suppresses {
        match resolve_sound_file(
            alerts.sound_file.as_deref(),
            config_dir,
            installed_data_dir,
            development_sound,
        ) {
            Ok(sound) => {
                let args = [sound.into_os_string()];
                if runner.spawn(OsStr::new("pw-play"), &args).is_err()
                    && let Err(error) = runner.spawn(OsStr::new("paplay"), &args)
                {
                    errors.push(format!(
                        "sound playback failed (pw-play and paplay): {error}"
                    ));
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(errors.join("; "))
    }
}

pub fn synthetic_session() -> Session {
    Session {
        provider: Provider::OpenCode,
        id: "alert-test".to_owned(),
        source: "local".to_owned(),
        source_instance_id: "local".to_owned(),
        source_label: None,
        expires_at: None,
        root_id: "alert-test".to_owned(),
        cwd: std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        kind: SessionKind::Main,
        status: crate::model::Status::Idle,
        pending: Default::default(),
        pid: None,
        process_start: None,
        updated_at: 0,
    }
}

pub fn resolve_sound_file(
    configured: Option<&Path>,
    config_dir: &Path,
    installed_data_dir: &Path,
    development_sound: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = configured {
        let expanded = expand_user_path(path);
        let resolved = if expanded.is_absolute() {
            expanded
        } else {
            config_dir.join(expanded)
        };
        if resolved.is_file() {
            return Ok(resolved);
        }
        bail!(
            "configured sound file does not exist: {}",
            resolved.display()
        );
    }
    let installed = installed_data_dir.join(SOUND_FILENAME);
    if installed.is_file() {
        return Ok(installed);
    }
    if let Some(path) = development_sound
        && path.is_file()
    {
        return Ok(path.to_path_buf());
    }
    Err(anyhow!(
        "could not find {SOUND_FILENAME}; run install.sh or configure idle_alerts.sound_file"
    ))
}

fn expand_user_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        home_dir()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        path.to_path_buf()
    }
}

fn is_local(session: &Session) -> bool {
    session.source == "local"
        && session.source_instance_id == "local"
        && session.expires_at.is_none()
}

fn notification_text(sessions: &[&Session]) -> (String, String) {
    if sessions.len() == 1 {
        let session = sessions[0];
        return (
            format!("{} is ready", provider_name(session.provider)),
            session_line(session, false),
        );
    }
    (
        format!("{} agent sessions are ready", sessions.len()),
        sessions
            .iter()
            .map(|session| session_line(session, true))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn session_line(session: &Session, include_provider: bool) -> String {
    let project = Path::new(&session.cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&session.cwd);
    let prefix = if include_provider {
        format!("{}: ", provider_name(session.provider))
    } else {
        String::new()
    };
    if is_local(session) {
        format!("{prefix}{project}")
    } else {
        let source = session
            .source_label
            .as_deref()
            .filter(|label| !label.is_empty())
            .unwrap_or(&session.source);
        format!(
            "{prefix}{project} ({source}/{})",
            session.source_instance_id
        )
    }
}

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenCode => "OpenCode",
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet, VecDeque};
    use std::sync::Mutex;

    use super::*;
    use crate::config::IdleAlerts;
    use crate::model::Status;

    #[derive(Default)]
    struct MockRunner {
        spawned: Mutex<Vec<Vec<String>>>,
        fail_spawn: Mutex<HashSet<String>>,
        captures: Mutex<Vec<Vec<String>>>,
        capture_results: Mutex<VecDeque<Result<String, String>>>,
    }

    impl CommandRunner for MockRunner {
        fn spawn(&self, program: &OsStr, args: &[OsString]) -> Result<()> {
            let program = program.to_string_lossy().into_owned();
            self.spawned.lock().unwrap().push(
                std::iter::once(program.clone())
                    .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
                    .collect(),
            );
            if self.fail_spawn.lock().unwrap().contains(&program) {
                bail!("{program} unavailable");
            }
            Ok(())
        }

        fn capture(&self, command: &[String], _timeout: Duration) -> Result<String> {
            self.captures.lock().unwrap().push(command.to_vec());
            self.capture_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(String::new()))
                .map_err(anyhow::Error::msg)
        }
    }

    fn session(id: &str, kind: SessionKind, remote: bool) -> Session {
        Session {
            provider: Provider::OpenCode,
            id: id.into(),
            source: if remote { "relay" } else { "local" }.into(),
            source_instance_id: if remote { "host-1" } else { "local" }.into(),
            source_label: remote.then(|| "Laptop".into()),
            expires_at: remote.then_some(100),
            root_id: id.into(),
            cwd: format!("/work/{id}"),
            kind,
            status: Status::Idle,
            pending: BTreeSet::new(),
            pid: None,
            process_start: None,
            updated_at: 1,
        }
    }

    fn config(notification: bool, sound: bool) -> Config {
        Config {
            idle_alerts: IdleAlerts {
                notification,
                sound,
                ..IdleAlerts::default()
            },
        }
    }

    #[test]
    fn disabled_delivery_runs_no_commands() {
        let runner = MockRunner::default();
        deliver_with(
            &[session("project", SessionKind::Main, false)],
            &Config::default(),
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            None,
        )
        .unwrap();
        assert!(runner.spawned.lock().unwrap().is_empty());
    }

    #[test]
    fn notification_uses_plain_expected_args_and_remote_context() {
        let runner = MockRunner::default();
        deliver_with(
            &[session("project<&", SessionKind::Main, true)],
            &config(true, false),
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            None,
        )
        .unwrap();
        assert_eq!(
            runner.spawned.lock().unwrap()[0],
            [
                "notify-send",
                "--app-name",
                "agent-session-status",
                "--urgency",
                "normal",
                "--icon",
                "dialog-information",
                "OpenCode is ready",
                "project<& (Laptop/host-1)",
            ]
        );
    }

    #[test]
    fn batching_scope_and_subagent_policy_filter_before_delivery() {
        let runner = MockRunner::default();
        let mut config = config(true, false);
        config.idle_alerts.scope = AlertScope::Remote;
        let sessions = [
            session("local", SessionKind::Main, false),
            session("remote", SessionKind::Main, true),
            session("agent", SessionKind::Agent, true),
        ];
        deliver_with(
            &sessions,
            &config,
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            None,
        )
        .unwrap();
        assert!(runner.spawned.lock().unwrap()[0][8].contains("remote"));

        config.idle_alerts.include_subagents = true;
        deliver_with(
            &sessions,
            &config,
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            None,
        )
        .unwrap();
        let commands = runner.spawned.lock().unwrap();
        assert_eq!(commands[1][7], "2 agent sessions are ready");
        assert!(commands[1][8].contains("remote"));
        assert!(commands[1][8].contains("agent"));
    }

    #[test]
    fn local_scope_rejects_remote_like_sessions() {
        let runner = MockRunner::default();
        let mut config = config(true, false);
        config.idle_alerts.scope = AlertScope::Local;
        deliver_with(
            &[session("remote", SessionKind::Main, true)],
            &config,
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            None,
        )
        .unwrap();
        assert!(runner.spawned.lock().unwrap().is_empty());
    }

    #[test]
    fn dnd_active_and_failure_suppress_sound_while_inactive_plays_once() {
        for result in [Ok(" yes \n".into()), Err("failed".into())] {
            let runner = MockRunner::default();
            let sound = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/agent-complete.wav");
            runner.capture_results.lock().unwrap().push_back(result);
            let mut config = config(false, true);
            config.idle_alerts.dnd_command = vec!["detect-dnd".into()];
            deliver_with(
                &[session("one", SessionKind::Main, false)],
                &config,
                Path::new("/config"),
                &runner,
                Path::new("/data"),
                Some(&sound),
            )
            .unwrap();
            assert!(runner.spawned.lock().unwrap().is_empty());
        }

        let runner = MockRunner::default();
        runner
            .capture_results
            .lock()
            .unwrap()
            .push_back(Ok("off".into()));
        let mut config = config(false, true);
        config.idle_alerts.dnd_command = vec!["detect-dnd".into()];
        let sound = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/agent-complete.wav");
        deliver_with(
            &[
                session("one", SessionKind::Main, false),
                session("two", SessionKind::Main, false),
            ],
            &config,
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            Some(&sound),
        )
        .unwrap();
        assert_eq!(runner.spawned.lock().unwrap().len(), 1);
    }

    #[test]
    fn falls_back_to_paplay_and_skips_dnd_without_a_command() {
        let runner = MockRunner::default();
        runner.fail_spawn.lock().unwrap().insert("pw-play".into());
        let sound = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/agent-complete.wav");
        deliver_with(
            &[session("one", SessionKind::Main, false)],
            &config(false, true),
            Path::new("/config"),
            &runner,
            Path::new("/data"),
            Some(&sound),
        )
        .unwrap();
        let commands = runner.spawned.lock().unwrap();
        assert_eq!(commands[0][0], "pw-play");
        assert_eq!(commands[1][0], "paplay");
        assert!(runner.captures.lock().unwrap().is_empty());
    }

    #[test]
    fn resolves_custom_installed_and_development_sound_paths() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(config_dir.join("custom.wav"), []).unwrap();
        assert_eq!(
            resolve_sound_file(Some(Path::new("custom.wav")), &config_dir, &data_dir, None)
                .unwrap(),
            config_dir.join("custom.wav")
        );
        std::fs::write(data_dir.join(SOUND_FILENAME), []).unwrap();
        assert_eq!(
            resolve_sound_file(None, &config_dir, &data_dir, None).unwrap(),
            data_dir.join(SOUND_FILENAME)
        );
        std::fs::remove_file(data_dir.join(SOUND_FILENAME)).unwrap();
        let development = dir.path().join("dev.wav");
        std::fs::write(&development, []).unwrap();
        assert_eq!(
            resolve_sound_file(None, &config_dir, &data_dir, Some(&development)).unwrap(),
            development
        );
    }
}
