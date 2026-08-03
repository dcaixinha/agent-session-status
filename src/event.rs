use std::collections::btree_map::Entry;

use serde_json::Value;

use crate::model::{Provider, Session, SessionKind, State, Status, session_key};
use crate::process::provider_process;
use crate::store::now;

pub fn apply_event(state: &mut State, provider: Provider, payload: &Value) {
    match provider {
        Provider::OpenCode => apply_opencode(state, payload),
        Provider::Claude => apply_hook_provider(state, provider, payload),
        Provider::Codex => apply_hook_provider(state, provider, payload),
    }
}

fn apply_opencode(state: &mut State, payload: &Value) {
    let event_type = string(payload, "type").unwrap_or_default();
    let properties = payload.get("properties").unwrap_or(&Value::Null);
    let fallback_cwd = string(payload, "instanceDirectory").unwrap_or_default();

    match event_type {
        "session.created" | "session.updated" => {
            let info = properties.get("info").unwrap_or(&Value::Null);
            let Some(id) = string(info, "id").or_else(|| string(properties, "sessionID")) else {
                return;
            };
            let parent = string(info, "parentID");
            let cwd = string(info, "directory").unwrap_or(fallback_cwd);
            let kind = if parent.is_some() {
                SessionKind::Agent
            } else {
                SessionKind::Main
            };
            let root = parent.unwrap_or(id);
            ensure_session(state, Provider::OpenCode, id, root, cwd, kind, Status::Idle);
        }
        "session.status" => {
            let Some(id) = string(properties, "sessionID") else {
                return;
            };
            let status_type = properties
                .get("status")
                .and_then(|status| string(status, "type"))
                .unwrap_or("idle");
            let status = match status_type {
                "busy" | "retry" => Status::Working,
                _ => Status::Idle,
            };
            let session = ensure_session(
                state,
                Provider::OpenCode,
                id,
                id,
                fallback_cwd,
                SessionKind::Main,
                status,
            );
            session.status = status;
            if status == Status::Idle {
                session.pending.clear();
            }
        }
        "session.idle" => {
            if let Some(id) = string(properties, "sessionID") {
                set_status(state, Provider::OpenCode, id, Status::Idle, true);
            }
        }
        "permission.asked" => {
            if let (Some(id), Some(request_id)) =
                (string(properties, "sessionID"), string(properties, "id"))
            {
                add_pending(
                    state,
                    Provider::OpenCode,
                    id,
                    &format!("permission:{request_id}"),
                    fallback_cwd,
                );
            }
        }
        "permission.replied" => {
            if let (Some(id), Some(request_id)) = (
                string(properties, "sessionID"),
                string(properties, "requestID"),
            ) {
                clear_pending(
                    state,
                    Provider::OpenCode,
                    id,
                    &format!("permission:{request_id}"),
                );
            }
        }
        "question.asked" => {
            if let (Some(id), Some(request_id)) =
                (string(properties, "sessionID"), string(properties, "id"))
            {
                add_pending(
                    state,
                    Provider::OpenCode,
                    id,
                    &format!("question:{request_id}"),
                    fallback_cwd,
                );
            }
        }
        "question.replied" | "question.rejected" => {
            if let (Some(id), Some(request_id)) = (
                string(properties, "sessionID"),
                string(properties, "requestID"),
            ) {
                clear_pending(
                    state,
                    Provider::OpenCode,
                    id,
                    &format!("question:{request_id}"),
                );
            }
        }
        "session.deleted" => {
            if let Some(id) = string(properties, "sessionID")
                .or_else(|| properties.get("info").and_then(|info| string(info, "id")))
            {
                remove_root(state, Provider::OpenCode, id);
            }
        }
        _ => {}
    }
}

fn apply_hook_provider(state: &mut State, provider: Provider, payload: &Value) {
    let Some(root_id) = string(payload, "session_id") else {
        return;
    };
    let event = string(payload, "hook_event_name").unwrap_or_default();
    let cwd = string(payload, "cwd").unwrap_or_default();
    let agent_id = string(payload, "agent_id");
    let id = agent_id
        .map(|agent| format!("{root_id}:{agent}"))
        .unwrap_or_else(|| root_id.to_owned());
    let kind = if agent_id.is_some() {
        SessionKind::Agent
    } else {
        SessionKind::Main
    };

    match event {
        "SessionStart" => {
            let source = string(payload, "source").unwrap_or("startup");
            let status = if source == "compact" {
                Status::Working
            } else {
                Status::Idle
            };
            let session = ensure_session(state, provider, &id, root_id, cwd, kind, status);
            session.status = status;
        }
        "SubagentStart" => {
            let session = ensure_session(
                state,
                provider,
                &id,
                root_id,
                cwd,
                SessionKind::Agent,
                Status::Working,
            );
            session.status = Status::Working;
        }
        "UserPromptSubmit" => set_status(state, provider, &id, Status::Working, true),
        "PermissionRequest" => {
            let request = string(payload, "tool_use_id")
                .or_else(|| string(payload, "turn_id"))
                .unwrap_or("current");
            add_pending(state, provider, &id, &format!("permission:{request}"), cwd);
        }
        "Notification" => match string(payload, "notification_type").unwrap_or_default() {
            "permission_prompt" | "agent_needs_input" => {
                add_pending(state, provider, &id, "permission:notification", cwd)
            }
            "idle_prompt" => set_status(state, provider, &id, Status::Idle, true),
            _ => {}
        },
        "PreToolUse" => {
            let tool = string(payload, "tool_name").unwrap_or_default();
            if matches!(tool, "AskUserQuestion" | "request_user_input") {
                let request = string(payload, "tool_use_id")
                    .or_else(|| string(payload, "turn_id"))
                    .unwrap_or("current");
                add_pending(state, provider, &id, &format!("question:{request}"), cwd);
            } else {
                set_status(state, provider, &id, Status::Working, false);
                clear_pending_prefix(state, provider, &id, "permission:");
            }
        }
        "PostToolUse" | "PostToolUseFailure" => {
            if let Some(request) = string(payload, "tool_use_id") {
                clear_pending(state, provider, &id, &format!("question:{request}"));
            }
            clear_pending_prefix(state, provider, &id, "permission:");
            set_status(state, provider, &id, Status::Working, false);
        }
        "Elicitation" => {
            let request = string(payload, "tool_use_id").unwrap_or("current");
            add_pending(state, provider, &id, &format!("elicitation:{request}"), cwd);
        }
        "ElicitationResult" => {
            clear_pending_prefix(state, provider, &id, "elicitation:");
            set_status(state, provider, &id, Status::Working, false);
        }
        "Stop" | "StopFailure" => set_status(state, provider, &id, Status::Idle, true),
        "SubagentStop" => {
            state.sessions.remove(&session_key(provider, &id));
        }
        "SessionEnd" => remove_root(state, provider, root_id),
        _ => {}
    }
}

fn ensure_session<'a>(
    state: &'a mut State,
    provider: Provider,
    id: &str,
    root_id: &str,
    cwd: &str,
    kind: SessionKind,
    initial_status: Status,
) -> &'a mut Session {
    let key = session_key(provider, id);
    let timestamp = now();
    let process = provider_process(provider);
    match state.sessions.entry(key) {
        Entry::Occupied(entry) => {
            let session = entry.into_mut();
            if !cwd.is_empty() {
                session.cwd = cwd.to_owned();
            }
            if let Some((pid, start)) = process {
                session.pid = Some(pid);
                session.process_start = Some(start);
            }
            session.updated_at = timestamp;
            session
        }
        Entry::Vacant(entry) => entry.insert(Session {
            provider,
            id: id.to_owned(),
            source: "local".to_owned(),
            source_instance_id: "local".to_owned(),
            source_label: None,
            expires_at: None,
            root_id: root_id.to_owned(),
            cwd: cwd.to_owned(),
            kind,
            status: initial_status,
            pending: Default::default(),
            pid: process.map(|value| value.0),
            process_start: process.map(|value| value.1),
            updated_at: timestamp,
        }),
    }
}

fn set_status(state: &mut State, provider: Provider, id: &str, status: Status, clear: bool) {
    let session = ensure_session(
        state,
        provider,
        id,
        id.split(':').next().unwrap_or(id),
        "",
        if id.contains(':') {
            SessionKind::Agent
        } else {
            SessionKind::Main
        },
        status,
    );
    session.status = status;
    if clear {
        session.pending.clear();
    }
}

fn add_pending(state: &mut State, provider: Provider, id: &str, request: &str, cwd: &str) {
    let session = ensure_session(
        state,
        provider,
        id,
        id.split(':').next().unwrap_or(id),
        cwd,
        if id.contains(':') {
            SessionKind::Agent
        } else {
            SessionKind::Main
        },
        Status::Working,
    );
    session.pending.insert(request.to_owned());
}

fn clear_pending(state: &mut State, provider: Provider, id: &str, request: &str) {
    if let Some(session) = state.sessions.get_mut(&session_key(provider, id)) {
        session.pending.remove(request);
        session.status = Status::Working;
        session.updated_at = now();
    }
}

fn clear_pending_prefix(state: &mut State, provider: Provider, id: &str, prefix: &str) {
    if let Some(session) = state.sessions.get_mut(&session_key(provider, id)) {
        session
            .pending
            .retain(|request| !request.starts_with(prefix));
        session.updated_at = now();
    }
}

fn remove_root(state: &mut State, provider: Provider, root_id: &str) {
    state
        .sessions
        .retain(|_, session| session.provider != provider || session.root_id != root_id);
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize)]
    struct FixtureStep {
        provider: Provider,
        session_key: String,
        event: Value,
        expected: Status,
    }

    #[test]
    fn provider_lifecycle_fixture() {
        let steps: Vec<FixtureStep> =
            serde_json::from_str(include_str!("../tests/fixtures/lifecycle.json")).unwrap();
        let mut state = State::default();

        for step in steps {
            apply_event(&mut state, step.provider, &step.event);
            assert_eq!(
                state.sessions[&step.session_key].effective_status(),
                step.expected
            );
        }
    }

    #[test]
    fn opencode_tracks_questions_until_reply() {
        let mut state = State::default();
        apply_event(
            &mut state,
            Provider::OpenCode,
            &json!({"type":"session.created","properties":{"info":{"id":"s1","directory":"/tmp/project"}}}),
        );
        apply_event(
            &mut state,
            Provider::OpenCode,
            &json!({"type":"question.asked","properties":{"id":"q1","sessionID":"s1"}}),
        );
        assert_eq!(
            state.sessions["opencode:s1"].effective_status(),
            Status::Waiting
        );

        apply_event(
            &mut state,
            Provider::OpenCode,
            &json!({"type":"question.replied","properties":{"requestID":"q1","sessionID":"s1"}}),
        );
        assert_eq!(
            state.sessions["opencode:s1"].effective_status(),
            Status::Working
        );
    }

    #[test]
    fn claude_question_clears_on_matching_tool_completion() {
        let mut state = State::default();
        apply_event(
            &mut state,
            Provider::Claude,
            &json!({"hook_event_name":"PreToolUse","session_id":"s1","cwd":"/tmp/project","tool_name":"AskUserQuestion","tool_use_id":"t1"}),
        );
        assert_eq!(
            state.sessions["claude:s1"].effective_status(),
            Status::Waiting
        );

        apply_event(
            &mut state,
            Provider::Claude,
            &json!({"hook_event_name":"PostToolUse","session_id":"s1","cwd":"/tmp/project","tool_name":"AskUserQuestion","tool_use_id":"t1"}),
        );
        assert_eq!(
            state.sessions["claude:s1"].effective_status(),
            Status::Working
        );
    }

    #[test]
    fn session_end_removes_agents_with_the_root() {
        let mut state = State::default();
        apply_event(
            &mut state,
            Provider::Codex,
            &json!({"hook_event_name":"SessionStart","session_id":"s1","cwd":"/tmp/project"}),
        );
        apply_event(
            &mut state,
            Provider::Codex,
            &json!({"hook_event_name":"SubagentStart","session_id":"s1","agent_id":"a1","cwd":"/tmp/project"}),
        );
        apply_event(
            &mut state,
            Provider::Codex,
            &json!({"hook_event_name":"SessionEnd","session_id":"s1","cwd":"/tmp/project"}),
        );
        assert!(state.sessions.is_empty());
    }
}
