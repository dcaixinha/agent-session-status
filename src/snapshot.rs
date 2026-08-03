use std::collections::BTreeSet;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::model::{Provider, Session, SessionKind, State, Status, remote_session_key};

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    source: String,
    ttl_seconds: u64,
    instances: Vec<SnapshotInstance>,
}

#[derive(Debug, Deserialize)]
struct SnapshotInstance {
    id: String,
    label: Option<String>,
    sessions: Option<Vec<SnapshotSession>>,
}

#[derive(Debug, Deserialize)]
struct SnapshotSession {
    id: String,
    provider: Provider,
    status: Status,
    cwd: String,
}

pub fn apply_snapshot_at(state: &mut State, snapshot: Snapshot, now: u64) -> Result<()> {
    validate(&snapshot)?;
    let expires_at = now
        .checked_add(snapshot.ttl_seconds)
        .ok_or_else(|| anyhow::anyhow!("ttl_seconds produces an expiry timestamp overflow"))?;
    let instance_ids: BTreeSet<_> = snapshot
        .instances
        .iter()
        .map(|instance| instance.id.as_str())
        .collect();

    state.sessions.retain(|_, session| {
        session.expires_at.is_none()
            || session.source != snapshot.source
            || instance_ids.contains(session.source_instance_id.as_str())
    });

    for instance in snapshot.instances {
        if let Some(sessions) = instance.sessions {
            state.sessions.retain(|_, session| {
                session.expires_at.is_none()
                    || session.source != snapshot.source
                    || session.source_instance_id != instance.id
            });
            for remote in sessions {
                let key =
                    remote_session_key(&snapshot.source, &instance.id, remote.provider, &remote.id);
                state.sessions.insert(
                    key,
                    Session {
                        provider: remote.provider,
                        id: remote.id.clone(),
                        source: snapshot.source.clone(),
                        source_instance_id: instance.id.clone(),
                        source_label: instance.label.clone(),
                        expires_at: Some(expires_at),
                        root_id: remote.id,
                        cwd: remote.cwd,
                        kind: SessionKind::Main,
                        status: remote.status,
                        pending: BTreeSet::new(),
                        pid: None,
                        process_start: None,
                        updated_at: now,
                    },
                );
            }
        } else {
            for session in state.sessions.values_mut().filter(|session| {
                session.expires_at.is_some()
                    && session.source == snapshot.source
                    && session.source_instance_id == instance.id
            }) {
                session.source_label.clone_from(&instance.label);
            }
        }
    }

    Ok(())
}

fn validate(snapshot: &Snapshot) -> Result<()> {
    if snapshot.source.trim().is_empty() {
        bail!("snapshot source must not be empty");
    }

    let mut instances = BTreeSet::new();
    for instance in &snapshot.instances {
        if instance.id.trim().is_empty() {
            bail!("snapshot instance id must not be empty");
        }
        if !instances.insert(&instance.id) {
            bail!("duplicate snapshot instance id {:?}", instance.id);
        }

        let mut sessions = BTreeSet::new();
        for session in instance.sessions.iter().flatten() {
            if session.id.trim().is_empty() {
                bail!(
                    "session id for instance {:?} must not be empty",
                    instance.id
                );
            }
            if !sessions.insert((session.provider, &session.id)) {
                bail!(
                    "duplicate session identity {}:{:?} for instance {:?}",
                    session.provider,
                    session.id,
                    instance.id
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn snapshot(value: serde_json::Value) -> Snapshot {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn replaces_present_instances_and_retains_absent_session_fields_without_renewal() {
        let mut state = State::default();
        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "remote",
                "ttl_seconds": 90,
                "instances": [
                    {"id": "replace", "label": "Old", "sessions": [{"id":"old", "provider":"claude", "status":"idle", "cwd":"/old"}]},
                    {"id": "retain", "label": "Old label", "sessions": [{"id":"kept", "provider":"codex", "status":"working", "cwd":"/kept"}]}
                ]
            })),
            100,
        )
        .unwrap();

        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "remote",
                "ttl_seconds": 200,
                "instances": [
                    {"id": "replace", "label": "New", "sessions": [{"id":"new", "provider":"opencode", "status":"waiting", "cwd":"/new"}]},
                    {"id": "retain", "label": "Updated label"}
                ]
            })),
            110,
        )
        .unwrap();

        assert_eq!(state.sessions.len(), 2);
        assert!(state.sessions.values().all(|session| session.id != "old"));
        let replaced = state
            .sessions
            .values()
            .find(|session| session.id == "new")
            .unwrap();
        assert_eq!(replaced.expires_at, Some(310));
        assert_eq!(replaced.kind, SessionKind::Main);
        assert_eq!(replaced.pid, None);
        let retained = state
            .sessions
            .values()
            .find(|session| session.id == "kept")
            .unwrap();
        assert_eq!(retained.expires_at, Some(190));
        assert_eq!(retained.source_label.as_deref(), Some("Updated label"));
    }

    #[test]
    fn empty_sessions_and_omitted_instances_remove_authoritative_state() {
        let mut state = State::default();
        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "remote", "ttl_seconds": 90,
                "instances": [
                    {"id":"empty-next", "sessions":[{"id":"one","provider":"claude","status":"idle","cwd":"/one"}]},
                    {"id":"omit-next", "sessions":[{"id":"two","provider":"codex","status":"idle","cwd":"/two"}]}
                ]
            })),
            10,
        )
        .unwrap();
        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "remote", "ttl_seconds": 90,
                "instances": [{"id":"empty-next", "sessions":[]}]
            })),
            20,
        )
        .unwrap();

        assert!(state.sessions.is_empty());
    }

    #[test]
    fn rejects_invalid_and_duplicate_identities_without_mutating_state() {
        let cases = [
            (
                json!({"source":"", "ttl_seconds":1, "instances":[]}),
                "source",
            ),
            (
                json!({"source":"x", "ttl_seconds":1, "instances":[{"id":" "}]}),
                "instance id",
            ),
            (
                json!({"source":"x", "ttl_seconds":1, "instances":[{"id":"a"},{"id":"a"}]}),
                "duplicate snapshot instance",
            ),
            (
                json!({"source":"x", "ttl_seconds":1, "instances":[{"id":"a","sessions":[{"id":"","provider":"claude","status":"idle","cwd":"/"}]}]}),
                "session id",
            ),
            (
                json!({"source":"x", "ttl_seconds":1, "instances":[{"id":"a","sessions":[{"id":"s","provider":"claude","status":"idle","cwd":"/"},{"id":"s","provider":"claude","status":"working","cwd":"/"}]}]}),
                "duplicate session identity",
            ),
        ];

        for (value, expected) in cases {
            let mut state = State::default();
            let error = apply_snapshot_at(&mut state, snapshot(value), 1).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert!(state.sessions.is_empty());
        }
    }

    #[test]
    fn same_session_identity_is_isolated_across_sources_and_instances() {
        let mut state = State::default();
        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "one", "ttl_seconds": 90,
                "instances": [
                    {"id":"host","sessions":[{"id":"same","provider":"claude","status":"idle","cwd":"/"}]},
                    {"id":"other","sessions":[{"id":"same","provider":"claude","status":"idle","cwd":"/"}]}
                ]
            })),
            1,
        )
        .unwrap();
        apply_snapshot_at(
            &mut state,
            snapshot(json!({
                "source": "two", "ttl_seconds": 90,
                "instances": [{"id":"host","sessions":[{"id":"same","provider":"claude","status":"idle","cwd":"/"}]}]
            })),
            1,
        )
        .unwrap();
        assert_eq!(state.sessions.len(), 3);
    }
}
