use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use crate::model::{Provider, SessionKind, State, Status};
use crate::workspace::{SessionLocation, WorkspaceMap};

#[derive(Clone, Copy, Debug)]
pub enum OutputFormat {
    Ironbar,
    Waybar,
    Details,
    Popup,
    Json,
}

pub fn render(
    state: &State,
    format: OutputFormat,
    provider: Option<Provider>,
    source: Option<&str>,
    group_source: bool,
    show_provider: bool,
    resolve_emacs: bool,
) -> Result<String> {
    let filtered;
    let state = if provider.is_some() || source.is_some() {
        filtered = State {
            sessions: state
                .sessions
                .iter()
                .filter(|(_, session)| {
                    provider.is_none_or(|provider| session.provider == provider)
                        && source.is_none_or(|source| session.source == source)
                })
                .map(|(key, session)| (key.clone(), session.clone()))
                .collect(),
        };
        &filtered
    } else {
        state
    };

    match format {
        OutputFormat::Ironbar if group_source => {
            Ok(render_source_group(state, source.unwrap_or_default()))
        }
        OutputFormat::Ironbar => Ok(render_ironbar(state, show_provider)),
        OutputFormat::Waybar => render_waybar(
            state,
            source.is_some(),
            group_source,
            show_provider,
            source.unwrap_or_default(),
            resolve_emacs,
        ),
        OutputFormat::Details => Ok(render_details(state, source.is_some())),
        OutputFormat::Popup => Ok(render_popup(state, source.is_some(), resolve_emacs)),
        OutputFormat::Json => Ok(serde_json::to_string(state)?),
    }
}

#[derive(Serialize)]
struct WaybarOutput {
    text: String,
    tooltip: String,
    class: Vec<&'static str>,
}

fn render_waybar(
    state: &State,
    source_filtered: bool,
    group_source: bool,
    show_provider: bool,
    source: &str,
    resolve_emacs: bool,
) -> Result<String> {
    let sessions = visible_sessions(state);
    if sessions.is_empty() {
        return Ok(serde_json::to_string(&WaybarOutput {
            text: String::new(),
            tooltip: String::new(),
            class: vec!["empty"],
        })?);
    }

    let highest = sessions
        .iter()
        .map(|session| session.effective_status())
        .max_by_key(|status| status.priority())
        .unwrap_or(Status::Idle);
    let mut class = vec![status_class(highest)];
    let mut main_statuses = sessions
        .iter()
        .filter(|session| session.kind == SessionKind::Main)
        .map(|session| session.effective_status());
    if let Some(first) = main_statuses.next()
        && main_statuses.any(|status| status != first)
    {
        class.push("mixed");
    }

    serde_json::to_string(&WaybarOutput {
        text: if group_source {
            render_source_group(state, source)
        } else {
            render_ironbar(state, show_provider)
        },
        tooltip: render_popup(state, source_filtered, resolve_emacs),
        class,
    })
    .map_err(Into::into)
}

const fn status_class(status: Status) -> &'static str {
    match status {
        Status::Waiting => "waiting",
        Status::Working => "working",
        Status::Idle => "idle",
    }
}

pub fn provider_status_color(state: &State, provider: Provider) -> String {
    let status = state
        .sessions
        .values()
        .filter(|session| session.provider == provider && session.kind == SessionKind::Main)
        .map(|session| session.effective_status())
        .max_by_key(|status| status.priority())
        .or_else(|| {
            state
                .sessions
                .values()
                .filter(|session| {
                    session.provider == provider
                        && session.kind == SessionKind::Agent
                        && session.effective_status() != Status::Idle
                })
                .map(|session| session.effective_status())
                .max_by_key(|status| status.priority())
        })
        .unwrap_or(Status::Idle);

    Colors::from_env().for_status(status).to_owned()
}

fn render_ironbar(state: &State, show_provider: bool) -> String {
    let colors = Colors::from_env();
    let display = DisplayConfig::from_env();
    let mut providers: BTreeMap<Provider, (Vec<Status>, Vec<Status>)> = BTreeMap::new();
    for session in state.sessions.values() {
        let status = session.effective_status();
        let entry = providers.entry(session.provider).or_default();
        match session.kind {
            SessionKind::Main => entry.0.push(status),
            SessionKind::Agent if status != Status::Idle => entry.1.push(status),
            SessionKind::Agent => {}
        }
    }

    providers
        .into_iter()
        .filter(|(_, (main, agents))| !main.is_empty() || !agents.is_empty())
        .map(|(provider, (main, agents))| {
            let highest = main
                .iter()
                .chain(&agents)
                .copied()
                .max_by_key(|status| status.priority())
                .unwrap_or(Status::Idle);
            let displayed_status = main
                .iter()
                .copied()
                .max_by_key(|status| status.priority())
                .unwrap_or(highest);
            let (main_count, partial) = match main.len() {
                0 | 1 => (String::new(), false),
                total => {
                    let matching = main
                        .iter()
                        .filter(|status| **status == displayed_status)
                        .count();
                    if matching == total {
                        (format!(" ({total})"), false)
                    } else {
                        (
                            format!(
                                " (<span foreground='{}' weight='bold'>{matching}</span>/{total})",
                                colors.for_status(displayed_status)
                            ),
                            true,
                        )
                    }
                }
            };
            let agent_count = if agents.is_empty() {
                String::new()
            } else {
                format!(" +{}", agents.len())
            };
            let status_label = display.status(displayed_status);
            let status_label = if partial {
                format!(
                    "<span foreground='{}'>{status_label}</span>",
                    colors.for_status(displayed_status)
                )
            } else {
                status_label
            };
            let provider_label = if show_provider {
                format!("{} ", provider.label())
            } else {
                String::new()
            };
            format!(
                "<span foreground='{}'>{}{}{}{}</span>",
                if partial {
                    colors.for_status(Status::Idle)
                } else {
                    colors.for_status(highest)
                },
                provider_label,
                status_label,
                main_count,
                agent_count
            )
        })
        .collect::<Vec<_>>()
        .join("  |  ")
}

fn render_source_group(state: &State, source: &str) -> String {
    let statuses: Vec<_> = state
        .sessions
        .values()
        .filter(|session| {
            session.kind == SessionKind::Main || session.effective_status() != Status::Idle
        })
        .map(|session| session.effective_status())
        .collect();
    let Some(highest) = statuses
        .iter()
        .copied()
        .max_by_key(|status| status.priority())
    else {
        return String::new();
    };
    let colors = Colors::from_env();
    let display = DisplayConfig::from_env();
    let source_label = std::env::var("AGENT_SESSION_STATUS_SOURCE_LABEL")
        .unwrap_or_else(|_| escape_markup(source));
    let total = statuses.len();
    let matching = statuses.iter().filter(|status| **status == highest).count();
    let count = match total {
        0 | 1 => String::new(),
        _ if matching == total => format!(" ({total})"),
        _ => format!(
            " (<span foreground='{}' weight='bold'>{matching}</span>/{total})",
            colors.for_status(highest)
        ),
    };

    format!(
        "<span foreground='{}'>{} {}{}</span>",
        if matching == total {
            colors.for_status(highest)
        } else {
            colors.for_status(Status::Idle)
        },
        source_label,
        if matching == total {
            display.status(highest)
        } else {
            format!(
                "<span foreground='{}'>{}</span>",
                colors.for_status(highest),
                display.status(highest)
            )
        },
        count
    )
}

fn render_details(state: &State, source_filtered: bool) -> String {
    let sessions = visible_sessions(state);
    if sessions.is_empty() {
        return "No open agent sessions".to_owned();
    }

    sessions
        .into_iter()
        .map(|session| {
            let label = session
                .cwd
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("?");
            let kind = match session.kind {
                crate::model::SessionKind::Main => "",
                crate::model::SessionKind::Agent => " (agent)",
            };
            if source_filtered && is_remote(session) {
                format!(
                    "{} - {}: {} - {}{}",
                    escape_markup(
                        session
                            .source_label
                            .as_deref()
                            .unwrap_or(&session.source_instance_id)
                    ),
                    session.provider,
                    session.effective_status(),
                    escape_markup(label),
                    kind
                )
            } else {
                format!(
                    "{}: {} - {}{}",
                    session.provider.label(),
                    session.effective_status(),
                    escape_markup(label),
                    kind
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_popup(state: &State, source_filtered: bool, resolve_emacs: bool) -> String {
    let sessions = visible_sessions(state);
    if sessions.is_empty() {
        return "No open sessions".to_owned();
    }

    let colors = Colors::from_env();
    let display = DisplayConfig::from_env();
    let workspaces = WorkspaceMap::detect();
    let locations = workspaces.locations(
        sessions.iter().filter_map(|session| session.pid),
        resolve_emacs,
    );
    sessions
        .into_iter()
        .map(|session| {
            let status = session.effective_status();
            let workspace = session
                .cwd
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("?");
            let kind = match session.kind {
                SessionKind::Main => "",
                SessionKind::Agent => " (agent)",
            };
            let remote = source_filtered && is_remote(session);
            let heading = if remote {
                escape_markup(
                    session
                        .source_label
                        .as_deref()
                        .unwrap_or(&session.source_instance_id),
                )
            } else {
                escape_markup(workspace)
            };
            let remote_identity = if remote {
                format!("{} - ", session.provider)
            } else {
                String::new()
            };
            let location = session
                .pid
                .and_then(|pid| locations.get(&pid))
                .map(|location| render_location(location, &colors))
                .unwrap_or_default();
            format!(
                "<span size='large' weight='bold'>{}</span>\n<span foreground='{}'>{}{} {}{}</span>{}\n<span size='small' foreground='{}'>{}</span>",
                heading,
                colors.for_status(status),
                remote_identity,
                display.icon(status),
                status,
                kind,
                location,
                colors.for_status(Status::Idle),
                escape_markup(&session.cwd)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_location(location: &SessionLocation, colors: &Colors) -> String {
    let (label, value) = match location {
        SessionLocation::Wayland(workspace) => ("Wayland workspace", workspace),
        SessionLocation::Emacs(perspective) => ("Emacs", perspective),
    };
    format!(
        "\n<span foreground='{}'>{label}: <b>{}</b></span>",
        colors.for_status(Status::Idle),
        escape_markup(value)
    )
}

fn is_remote(session: &crate::model::Session) -> bool {
    session.expires_at.is_some()
        || session.source != "local"
        || session.source_instance_id != "local"
}

fn visible_sessions(state: &State) -> Vec<&crate::model::Session> {
    state
        .sessions
        .values()
        .filter(|session| {
            session.kind == SessionKind::Main || session.effective_status() != Status::Idle
        })
        .collect()
}

fn escape_markup(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct Colors {
    waiting: String,
    working: String,
    idle: String,
}

impl Colors {
    fn from_env() -> Self {
        Self {
            waiting: env_color("WAITING", "#e0af68"),
            working: env_color("WORKING", "#9ece6a"),
            idle: env_color("IDLE", "#7f849c"),
        }
    }

    fn for_status(&self, status: Status) -> &str {
        match status {
            Status::Waiting => &self.waiting,
            Status::Working => &self.working,
            Status::Idle => &self.idle,
        }
    }
}

fn env_color(state: &str, default: &str) -> String {
    std::env::var(format!("AGENT_SESSION_STATUS_COLOR_{state}"))
        .unwrap_or_else(|_| default.to_owned())
}

#[derive(Clone, Copy)]
enum DisplayMode {
    Icons,
    Text,
    Both,
}

struct DisplayConfig {
    mode: DisplayMode,
    waiting: String,
    working: String,
    idle: String,
}

impl DisplayConfig {
    fn from_env() -> Self {
        let mode = match std::env::var("AGENT_SESSION_STATUS_DISPLAY")
            .unwrap_or_else(|_| "icons".to_owned())
            .to_lowercase()
            .as_str()
        {
            "text" => DisplayMode::Text,
            "both" => DisplayMode::Both,
            _ => DisplayMode::Icons,
        };

        Self {
            mode,
            waiting: env_icon(
                "WAITING",
                "<span font_family='Font Awesome 7 Free Solid'>\u{f059}</span>",
            ),
            working: env_icon(
                "WORKING",
                "<span font_family='Font Awesome 7 Free Solid'>\u{f085}</span>",
            ),
            idle: env_icon(
                "IDLE",
                "<span font_family='Font Awesome 7 Free Solid'>\u{f28b}</span>",
            ),
        }
    }

    fn status(&self, status: Status) -> String {
        let icon = self.icon(status);

        match self.mode {
            DisplayMode::Icons => icon.to_owned(),
            DisplayMode::Text => status.to_string(),
            DisplayMode::Both => format!("{icon} {status}"),
        }
    }

    fn icon(&self, status: Status) -> &str {
        match status {
            Status::Waiting => &self.waiting,
            Status::Working => &self.working,
            Status::Idle => &self.idle,
        }
    }
}

fn env_icon(state: &str, default: &str) -> String {
    std::env::var(format!("AGENT_SESSION_STATUS_ICON_{state}"))
        .unwrap_or_else(|_| default.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::model::{Session, session_key};

    use super::*;

    fn session(provider: Provider, id: &str, kind: SessionKind, status: Status) -> Session {
        Session {
            provider,
            id: id.to_owned(),
            source: "local".to_owned(),
            source_instance_id: "local".to_owned(),
            source_label: None,
            expires_at: None,
            root_id: id.to_owned(),
            cwd: format!("/tmp/{id}"),
            kind,
            status,
            pending: BTreeSet::new(),
            pid: None,
            process_start: None,
            updated_at: 0,
        }
    }

    #[test]
    fn renders_provider_segments_and_active_agent_count() {
        let mut state = State::default();
        state.sessions.insert(
            session_key(Provider::OpenCode, "one"),
            session(
                Provider::OpenCode,
                "one",
                SessionKind::Main,
                Status::Working,
            ),
        );
        state.sessions.insert(
            session_key(Provider::OpenCode, "agent"),
            session(
                Provider::OpenCode,
                "agent",
                SessionKind::Agent,
                Status::Waiting,
            ),
        );
        state.sessions.insert(
            session_key(Provider::Claude, "two"),
            session(Provider::Claude, "two", SessionKind::Main, Status::Idle),
        );

        let output = render_ironbar(&state, true);
        assert!(
            output.contains("OC <span font_family='Font Awesome 7 Free Solid'>\u{f085}</span> +1")
        );
        assert!(
            output.contains("Claude <span font_family='Font Awesome 7 Free Solid'>\u{f28b}</span>")
        );
    }

    #[test]
    fn hides_idle_agents_from_the_bar() {
        let mut state = State::default();
        state.sessions.insert(
            session_key(Provider::Codex, "agent"),
            session(Provider::Codex, "agent", SessionKind::Agent, Status::Idle),
        );

        assert_eq!(render_ironbar(&state, true), "");
        assert_eq!(render_details(&state, false), "No open agent sessions");
        assert_eq!(render_popup(&state, false, false), "No open sessions");
        assert!(!state.sessions.values().any(|session| {
            session.kind == SessionKind::Main || session.effective_status() != Status::Idle
        }));
    }

    #[test]
    fn renders_partial_and_uniform_session_counts() {
        let mut state = State::default();
        state.sessions.insert(
            session_key(Provider::OpenCode, "one"),
            session(
                Provider::OpenCode,
                "one",
                SessionKind::Main,
                Status::Working,
            ),
        );
        state.sessions.insert(
            session_key(Provider::OpenCode, "two"),
            session(Provider::OpenCode, "two", SessionKind::Main, Status::Idle),
        );

        assert!(
            render_ironbar(&state, true).contains(
                "OC <span foreground='#9ece6a'><span font_family='Font Awesome 7 Free Solid'>\u{f085}</span></span> (<span foreground='#9ece6a' weight='bold'>1</span>/2)"
            )
        );

        state.sessions.get_mut("opencode:two").unwrap().status = Status::Working;
        assert!(
            render_ironbar(&state, true)
                .contains("OC <span font_family='Font Awesome 7 Free Solid'>\u{f085}</span> (2)")
        );

        for session in state.sessions.values_mut() {
            session.status = Status::Idle;
        }
        assert!(
            render_ironbar(&state, true)
                .contains("OC <span font_family='Font Awesome 7 Free Solid'>\u{f28b}</span> (2)")
        );
    }

    #[test]
    fn popup_filters_provider_and_shows_workspace_details() {
        let mut state = State::default();
        state.sessions.insert(
            session_key(Provider::OpenCode, "one"),
            session(
                Provider::OpenCode,
                "one",
                SessionKind::Main,
                Status::Working,
            ),
        );
        state.sessions.insert(
            session_key(Provider::Claude, "two"),
            session(Provider::Claude, "two", SessionKind::Main, Status::Idle),
        );

        let output = render(
            &state,
            OutputFormat::Popup,
            Some(Provider::OpenCode),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        assert!(output.contains("<span size='large' weight='bold'>one</span>"));
        assert!(output.contains("working"));
        assert!(!output.contains("two"));
    }

    #[test]
    fn popup_location_escapes_emacs_perspective_markup() {
        let output = render_location(
            &SessionLocation::Emacs("~/project<&".to_owned()),
            &Colors::from_env(),
        );

        assert!(output.contains("Emacs: <b>~/project&lt;&amp;</b>"));
    }

    #[test]
    fn can_hide_provider_name_for_image_based_modules() {
        let mut state = State::default();
        state.sessions.insert(
            session_key(Provider::OpenCode, "one"),
            session(Provider::OpenCode, "one", SessionKind::Main, Status::Idle),
        );

        let output = render_ironbar(&state, false);
        assert!(!output.contains("OC"));
        assert!(output.contains("\u{f28b}"));
    }

    #[test]
    fn source_filter_composes_with_provider_and_groups_one_segment() {
        let mut state = State::default();
        let mut working = session(Provider::Claude, "one", SessionKind::Main, Status::Working);
        working.source = "fleet".to_owned();
        working.source_instance_id = "host-one".to_owned();
        working.source_label = Some("Build host".to_owned());
        working.expires_at = Some(100);
        let mut idle = session(Provider::Codex, "two", SessionKind::Main, Status::Idle);
        idle.source = "fleet".to_owned();
        idle.source_instance_id = "host-two".to_owned();
        idle.expires_at = Some(100);
        state.sessions.insert("remote-one".to_owned(), working);
        state.sessions.insert("remote-two".to_owned(), idle);
        state.sessions.insert(
            "other".to_owned(),
            session(
                Provider::Claude,
                "other",
                SessionKind::Main,
                Status::Waiting,
            ),
        );

        let grouped = render(
            &state,
            OutputFormat::Ironbar,
            None,
            Some("fleet"),
            true,
            true,
            false,
        )
        .unwrap();
        assert!(grouped.contains("fleet"));
        assert!(grouped.contains("1</span>/2"));
        assert!(!grouped.contains("Claude"));
        assert_eq!(grouped.matches("fleet").count(), 1);

        let details = render(
            &state,
            OutputFormat::Details,
            Some(Provider::Claude),
            Some("fleet"),
            false,
            true,
            false,
        )
        .unwrap();
        assert!(details.contains("Build host - claude: working"));
        assert!(!details.contains("host-two"));
        assert!(!details.contains("other"));
    }

    #[test]
    fn source_filtered_remote_popup_identifies_instance_and_provider() {
        let mut remote = session(Provider::Codex, "one", SessionKind::Main, Status::Idle);
        remote.source = "fleet".to_owned();
        remote.source_instance_id = "host-one".to_owned();
        remote.source_label = Some("Display host".to_owned());
        remote.expires_at = Some(100);
        let state = State {
            sessions: [("remote".to_owned(), remote)].into_iter().collect(),
        };

        let output = render(
            &state,
            OutputFormat::Popup,
            None,
            Some("fleet"),
            false,
            true,
            false,
        )
        .unwrap();
        assert!(output.contains("weight='bold'>Display host</span>"));
        assert!(output.contains("codex -"));
    }

    #[test]
    fn renders_one_line_waybar_json_with_tooltip_and_classes() {
        let mut working = session(
            Provider::OpenCode,
            "one",
            SessionKind::Main,
            Status::Working,
        );
        working.cwd = "/tmp/project<&\"name".to_owned();
        let idle = session(Provider::OpenCode, "two", SessionKind::Main, Status::Idle);
        let mut waiting_agent = session(
            Provider::OpenCode,
            "agent",
            SessionKind::Agent,
            Status::Working,
        );
        waiting_agent.pending.insert("question:one".to_owned());
        let state = State {
            sessions: [
                ("working".to_owned(), working),
                ("idle".to_owned(), idle),
                ("agent".to_owned(), waiting_agent),
            ]
            .into_iter()
            .collect(),
        };

        let output = render(
            &state,
            OutputFormat::Waybar,
            Some(Provider::OpenCode),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(output.lines().count(), 1);

        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(value["text"].as_str().unwrap().contains("OC"));
        assert!(
            value["tooltip"]
                .as_str()
                .unwrap()
                .contains("project&lt;&amp;\"name")
        );
        assert!(value["tooltip"].as_str().unwrap().contains('\n'));
        assert_eq!(value["class"], serde_json::json!(["waiting", "mixed"]));
    }

    #[test]
    fn waybar_empty_output_stays_valid_and_grouped_sources_share_the_adapter() {
        let mut idle_agent = session(Provider::Claude, "agent", SessionKind::Agent, Status::Idle);
        idle_agent.source = "fleet".to_owned();
        let empty = State {
            sessions: [("agent".to_owned(), idle_agent)].into_iter().collect(),
        };
        let output = render(
            &empty,
            OutputFormat::Waybar,
            None,
            Some("fleet"),
            true,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap(),
            serde_json::json!({"text": "", "tooltip": "", "class": ["empty"]})
        );

        let mut remote = session(Provider::Claude, "main", SessionKind::Main, Status::Working);
        remote.source = "fleet".to_owned();
        remote.source_instance_id = "host-one".to_owned();
        remote.source_label = Some("Build host".to_owned());
        remote.expires_at = Some(100);
        let mut hidden_agent = session(
            Provider::Claude,
            "idle-agent",
            SessionKind::Agent,
            Status::Idle,
        );
        hidden_agent.source = "fleet".to_owned();
        let grouped = State {
            sessions: [
                ("remote".to_owned(), remote),
                ("idle-agent".to_owned(), hidden_agent),
            ]
            .into_iter()
            .collect(),
        };
        let output = render(
            &grouped,
            OutputFormat::Waybar,
            None,
            Some("fleet"),
            true,
            true,
            false,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(value["text"].as_str().unwrap().contains("fleet"));
        assert!(!value["text"].as_str().unwrap().contains("(2)"));
        assert!(value["tooltip"].as_str().unwrap().contains("Build host"));
        assert!(!value["tooltip"].as_str().unwrap().contains("idle-agent"));
        assert_eq!(value["class"], serde_json::json!(["working"]));
    }
}
