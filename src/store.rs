use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use notify::{EventKind, RecursiveMode, Watcher};

use crate::model::{Provider, Session, State, Status};
use crate::process::is_same_process;
use crate::render::{OutputFormat, render};

const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Store {
    dir: PathBuf,
    state_path: PathBuf,
    lock_path: PathBuf,
}

impl Store {
    pub fn new(override_dir: Option<PathBuf>) -> Result<Self> {
        let dir = override_dir.unwrap_or_else(default_state_dir);
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create state directory {}", dir.display()))?;
        Ok(Self {
            state_path: dir.join("state.json"),
            lock_path: dir.join("state.lock"),
            dir,
        })
    }

    pub fn update(
        &self,
        apply: impl FnOnce(&mut State, u64) -> Result<()>,
    ) -> Result<Vec<Session>> {
        let timestamp = now();
        self.with_lock(|state| {
            prune_at(state, timestamp);
            let baseline = state.clone();
            apply(state, timestamp)?;
            prune_at(state, timestamp);
            Ok(idle_transitions(&baseline, state))
        })
    }

    pub fn load_and_prune(&self) -> Result<State> {
        self.with_lock(|state| {
            prune(state);
            Ok(state.clone())
        })
    }

    pub fn clear(&self) -> Result<()> {
        self.with_lock(|state| {
            state.sessions.clear();
            Ok(())
        })
    }

    pub fn watch(
        &self,
        format: OutputFormat,
        provider: Option<Provider>,
        source: Option<String>,
        group_source: bool,
        show_provider: bool,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })?;
        watcher.watch(&self.dir, RecursiveMode::NonRecursive)?;

        self.print(
            format,
            provider,
            source.as_deref(),
            group_source,
            show_provider,
        )?;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok(event))
                    if !matches!(event.kind, EventKind::Access(_))
                        && event.paths.iter().any(|path| path == &self.state_path) =>
                {
                    // Coalesce atomic-write event bursts before reading.
                    std::thread::sleep(Duration::from_millis(15));
                    while rx.try_recv().is_ok() {}
                    self.print(
                        format,
                        provider,
                        source.as_deref(),
                        group_source,
                        show_provider,
                    )?;
                }
                Ok(Ok(_)) | Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => self.print(
                    format,
                    provider,
                    source.as_deref(),
                    group_source,
                    show_provider,
                )?,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn print(
        &self,
        format: OutputFormat,
        provider: Option<Provider>,
        source: Option<&str>,
        group_source: bool,
        show_provider: bool,
    ) -> Result<()> {
        println!(
            "{}",
            render(
                &self.load_and_prune()?,
                format,
                provider,
                source,
                group_source,
                show_provider,
            )?
        );
        std::io::stdout().flush()?;
        Ok(())
    }

    fn with_lock<T>(&self, operation: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        lock.lock_exclusive()?;

        let mut state = read_state(&self.state_path)?;
        let original = state.clone();
        let result = operation(&mut state)?;
        if state != original {
            write_state(&self.state_path, &state)?;
        }
        lock.unlock()?;
        Ok(result)
    }
}

fn idle_transitions(before: &State, after: &State) -> Vec<Session> {
    before
        .sessions
        .iter()
        .filter_map(|(key, old)| {
            let new = after.sessions.get(key)?;
            (old.effective_status() != Status::Idle && new.effective_status() == Status::Idle)
                .then(|| new.clone())
        })
        .collect()
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("agent-session-status")
}

fn read_state(path: &Path) -> Result<State> {
    if !path.exists() {
        return Ok(State::default());
    }
    let file = File::open(path)?;
    serde_json::from_reader(file).context("failed to parse session state")
}

fn write_state(path: &Path, state: &State) -> Result<()> {
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let file = File::create(&temp)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, state)?;
        writer.flush()?;
    }
    fs::rename(temp, path)?;
    Ok(())
}

fn prune(state: &mut State) {
    prune_at(state, now());
}

fn prune_at(state: &mut State, now: u64) {
    state.sessions.retain(|_, session| {
        if let Some(expires_at) = session.expires_at {
            return expires_at > now;
        }
        if let Some(pid) = session.pid {
            return is_same_process(pid, session.process_start);
        }
        now.saturating_sub(session.updated_at) <= STALE_AFTER.as_secs()
    });
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::model::{Session, SessionKind, Status};

    use super::*;

    fn session(id: &str, status: Status) -> Session {
        Session {
            provider: Provider::Claude,
            id: id.to_owned(),
            source: "local".to_owned(),
            source_instance_id: "local".to_owned(),
            source_label: None,
            expires_at: None,
            root_id: id.to_owned(),
            cwd: format!("/tmp/{id}"),
            kind: SessionKind::Main,
            status,
            pending: BTreeSet::new(),
            pid: None,
            process_start: None,
            updated_at: now(),
        }
    }

    #[test]
    fn expiry_is_pruned_before_pid_and_stale_age_rules() {
        let mut state = State::default();
        state.sessions.insert(
            "expired".to_owned(),
            Session {
                provider: Provider::Claude,
                id: "expired".to_owned(),
                source: "remote".to_owned(),
                source_instance_id: "host".to_owned(),
                source_label: None,
                expires_at: Some(100),
                root_id: "expired".to_owned(),
                cwd: "/tmp".to_owned(),
                kind: SessionKind::Main,
                status: Status::Idle,
                pending: BTreeSet::new(),
                pid: Some(std::process::id()),
                process_start: None,
                updated_at: 100,
            },
        );
        state.sessions.insert(
            "unexpired".to_owned(),
            Session {
                provider: Provider::Codex,
                id: "unexpired".to_owned(),
                source: "remote".to_owned(),
                source_instance_id: "host".to_owned(),
                source_label: None,
                expires_at: Some(101),
                root_id: "unexpired".to_owned(),
                cwd: "/tmp".to_owned(),
                kind: SessionKind::Main,
                status: Status::Idle,
                pending: BTreeSet::new(),
                pid: None,
                process_start: None,
                updated_at: 0,
            },
        );

        prune_at(&mut state, 100);
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions.contains_key("unexpired"));
    }

    #[test]
    fn update_detects_effective_idle_transitions_and_batches_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(Some(dir.path().into())).unwrap();
        store
            .update(|state, _| {
                state
                    .sessions
                    .insert("claude:working".into(), session("working", Status::Working));
                let mut waiting = session("waiting", Status::Idle);
                waiting.pending.insert("question:1".into());
                state.sessions.insert("claude:waiting".into(), waiting);
                state.sessions.insert(
                    "claude:already-idle".into(),
                    session("already-idle", Status::Idle),
                );
                Ok(())
            })
            .unwrap();

        let transitions = store
            .update(|state, _| {
                state.sessions.get_mut("claude:working").unwrap().status = Status::Idle;
                state
                    .sessions
                    .get_mut("claude:waiting")
                    .unwrap()
                    .pending
                    .clear();
                state
                    .sessions
                    .insert("claude:new-idle".into(), session("new-idle", Status::Idle));
                state.sessions.remove("claude:already-idle");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            transitions
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["waiting", "working"]
        );

        let repeated = store.update(|_, _| Ok(())).unwrap();
        assert!(repeated.is_empty());
    }

    #[test]
    fn expiry_during_update_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(Some(dir.path().into())).unwrap();
        store
            .with_lock(|state| {
                let mut expired = session("expired", Status::Working);
                expired.expires_at = Some(1);
                state.sessions.insert("claude:expired".into(), expired);
                Ok(())
            })
            .unwrap();
        assert!(store.update(|_, _| Ok(())).unwrap().is_empty());
    }

    #[test]
    fn snapshot_polling_alerts_once_for_existing_remote_session() {
        use crate::snapshot::{Snapshot, apply_snapshot_at};

        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(Some(dir.path().into())).unwrap();
        let parse = |status: &str| {
            serde_json::from_value::<Snapshot>(serde_json::json!({
                "source":"relay", "ttl_seconds":90,
                "instances":[{"id":"host","sessions":[{
                    "id":"one", "provider":"opencode", "status":status,
                    "cwd":"/work/project"
                }]}]
            }))
            .unwrap()
        };
        assert!(
            store
                .update(|state, at| apply_snapshot_at(state, parse("working"), at))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .update(|state, at| apply_snapshot_at(state, parse("idle"), at))
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .update(|state, at| apply_snapshot_at(state, parse("idle"), at))
                .unwrap()
                .is_empty()
        );
        let omitted: Snapshot = serde_json::from_value(serde_json::json!({
            "source":"relay", "ttl_seconds":90, "instances":[]
        }))
        .unwrap();
        assert!(
            store
                .update(|state, at| apply_snapshot_at(state, omitted, at))
                .unwrap()
                .is_empty()
        );
    }
}
