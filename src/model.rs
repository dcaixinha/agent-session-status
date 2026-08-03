use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    OpenCode,
    Claude,
    Codex,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenCode => "OC",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idle,
    Working,
    Waiting,
}

impl Status {
    pub fn priority(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working => 1,
            Self::Waiting => 2,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::Working => f.write_str("working"),
            Self::Waiting => f.write_str("waiting"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Main,
    Agent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Session {
    pub provider: Provider,
    pub id: String,
    #[serde(default = "local_source")]
    pub source: String,
    #[serde(default = "local_source")]
    pub source_instance_id: String,
    #[serde(default)]
    pub source_label: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub root_id: String,
    pub cwd: String,
    pub kind: SessionKind,
    pub status: Status,
    #[serde(default)]
    pub pending: BTreeSet<String>,
    pub pid: Option<u32>,
    pub process_start: Option<u64>,
    pub updated_at: u64,
}

impl Session {
    pub fn effective_status(&self) -> Status {
        if self.pending.is_empty() {
            self.status
        } else {
            Status::Waiting
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct State {
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
}

pub fn session_key(provider: Provider, id: &str) -> String {
    format!("{}:{id}", provider.key())
}

pub fn remote_session_key(source: &str, instance_id: &str, provider: Provider, id: &str) -> String {
    format!(
        "remote:{}:{source}:{}:{instance_id}:{}:{}:{id}",
        source.len(),
        instance_id.len(),
        provider.key(),
        id.len()
    )
}

fn local_source() -> String {
    "local".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_sessions_deserialize_with_local_source_defaults() {
        let state: State = serde_json::from_str(
            r#"{"sessions":{"claude:one":{"provider":"claude","id":"one","root_id":"one","cwd":"/tmp","kind":"main","status":"idle","pid":null,"process_start":null,"updated_at":1}}}"#,
        )
        .unwrap();
        let session = &state.sessions["claude:one"];

        assert_eq!(session.source, "local");
        assert_eq!(session.source_instance_id, "local");
        assert_eq!(session.source_label, None);
        assert_eq!(session.expires_at, None);
    }

    #[test]
    fn remote_keys_isolate_delimiter_collisions_and_sources() {
        let first = remote_session_key("a:b", "c", Provider::Claude, "same");
        let delimiter_collision = remote_session_key("a", "b:c", Provider::Claude, "same");
        let other_source = remote_session_key("other", "c", Provider::Claude, "same");
        let other_provider = remote_session_key("a:b", "c", Provider::Codex, "same");

        assert_ne!(first, delimiter_collision);
        assert_ne!(first, other_source);
        assert_ne!(first, other_provider);
    }
}
