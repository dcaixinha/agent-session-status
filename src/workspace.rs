use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use crate::process::{ancestor_pids, process_name};

const EMACS_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionLocation {
    Wayland(String),
    Emacs(String),
}

#[derive(Default)]
pub struct WorkspaceMap {
    by_pid: BTreeMap<u32, Vec<WindowLocation>>,
}

impl WorkspaceMap {
    pub fn detect() -> Self {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            return Self::from_hyprland();
        }
        Self::default()
    }

    pub fn locations(
        &self,
        pids: impl IntoIterator<Item = u32>,
        resolve_emacs: bool,
    ) -> BTreeMap<u32, SessionLocation> {
        self.locations_with(pids, resolve_emacs, &SystemEmacsResolver)
    }

    fn from_hyprland() -> Self {
        let output = Command::new("hyprctl").args(["-j", "clients"]).output();
        let Ok(output) = output else {
            return Self::default();
        };
        if !output.status.success() {
            return Self::default();
        }

        let clients: Vec<HyprlandClient> =
            serde_json::from_slice(&output.stdout).unwrap_or_default();
        let labels = workspace_labels();
        Self::from_clients(clients, &labels)
    }

    fn from_clients(clients: Vec<HyprlandClient>, labels: &BTreeMap<String, String>) -> Self {
        let mut by_pid: BTreeMap<u32, Vec<WindowLocation>> = BTreeMap::new();
        for client in clients.into_iter().filter(|client| client.mapped) {
            let name = client.workspace.name;
            let workspace = labels.get(&name).cloned().unwrap_or(name);
            by_pid.entry(client.pid).or_default().push(WindowLocation {
                title: client.title,
                workspace,
            });
        }
        Self { by_pid }
    }

    fn locations_with(
        &self,
        pids: impl IntoIterator<Item = u32>,
        resolve_emacs: bool,
        resolver: &dyn EmacsResolver,
    ) -> BTreeMap<u32, SessionLocation> {
        let mut locations = BTreeMap::new();
        let mut pending = Vec::new();
        for pid in pids.into_iter().collect::<BTreeSet<_>>() {
            let ancestors = ancestor_pids(pid);
            let Some((index, windows)) = self.for_ancestors(&ancestors) else {
                continue;
            };
            let workspaces = unique_workspaces(windows);
            if workspaces.len() == 1 {
                locations.insert(pid, SessionLocation::Wayland(workspaces[0].to_owned()));
            } else if resolve_emacs
                && process_name(ancestors[index]).as_deref() == Some("emacs")
                && let Some(shell_pid) = index.checked_sub(1).map(|index| ancestors[index])
            {
                pending.push(PendingEmacsLocation {
                    session_pid: pid,
                    shell_pid,
                    emacs_pid: ancestors[index],
                    windows: windows.to_vec(),
                });
            }
        }
        locations.extend(resolve_emacs_locations(&pending, resolve_emacs, resolver));
        locations
    }

    fn for_ancestors<'a>(&'a self, ancestors: &[u32]) -> Option<(usize, &'a [WindowLocation])> {
        ancestors.iter().enumerate().find_map(|(index, pid)| {
            self.by_pid
                .get(pid)
                .map(|windows| (index, windows.as_slice()))
        })
    }
}

fn unique_workspaces(windows: &[WindowLocation]) -> Vec<&str> {
    let mut workspaces: Vec<_> = windows
        .iter()
        .map(|window| window.workspace.as_str())
        .collect();
    workspaces.sort_unstable();
    workspaces.dedup();
    workspaces
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowLocation {
    title: String,
    workspace: String,
}

struct PendingEmacsLocation {
    session_pid: u32,
    shell_pid: u32,
    emacs_pid: u32,
    windows: Vec<WindowLocation>,
}

#[derive(Clone, Debug, Deserialize)]
struct EmacsProcess {
    shell_pid: u32,
    emacs_pid: u32,
    #[serde(default)]
    frames: Vec<String>,
    #[serde(default)]
    perspectives: Vec<String>,
}

trait EmacsResolver {
    fn query(&self, shell_pids: &[u32]) -> Vec<EmacsProcess>;
}

struct SystemEmacsResolver;

impl EmacsResolver for SystemEmacsResolver {
    fn query(&self, shell_pids: &[u32]) -> Vec<EmacsProcess> {
        query_emacs(shell_pids).unwrap_or_default()
    }
}

fn resolve_emacs_locations(
    pending: &[PendingEmacsLocation],
    enabled: bool,
    resolver: &dyn EmacsResolver,
) -> BTreeMap<u32, SessionLocation> {
    if !enabled || pending.is_empty() {
        return BTreeMap::new();
    }

    let shell_pids: Vec<_> = pending.iter().map(|location| location.shell_pid).collect();
    let responses = resolver.query(&shell_pids);
    pending
        .iter()
        .filter_map(|location| {
            let response = responses.iter().find(|response| {
                response.shell_pid == location.shell_pid && response.emacs_pid == location.emacs_pid
            })?;
            visible_workspace(response, &location.windows)
                .map(SessionLocation::Wayland)
                .or_else(|| {
                    if !response.frames.is_empty() {
                        return None;
                    }
                    let perspectives = normalized_perspectives(&response.perspectives);
                    (!perspectives.is_empty())
                        .then(|| SessionLocation::Emacs(perspectives.join(", ")))
                })
                .map(|resolved| (location.session_pid, resolved))
        })
        .collect()
}

fn visible_workspace(response: &EmacsProcess, windows: &[WindowLocation]) -> Option<String> {
    let mut workspaces: Vec<_> = response
        .frames
        .iter()
        .flat_map(|frame| windows.iter().filter(move |window| window.title == *frame))
        .map(|window| window.workspace.as_str())
        .collect();
    workspaces.sort_unstable();
    workspaces.dedup();
    (workspaces.len() == 1).then(|| workspaces[0].to_owned())
}

fn normalized_perspectives(perspectives: &[String]) -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut normalized: Vec<_> = perspectives
        .iter()
        .map(|perspective| {
            let path = PathBuf::from(perspective);
            let display = home
                .as_deref()
                .and_then(|home| path.strip_prefix(home).ok())
                .map(|relative| format!("~/{}", relative.display()))
                .unwrap_or_else(|| perspective.clone());
            let trimmed = display.trim_end_matches('/');
            if trimmed.is_empty() {
                "/".to_owned()
            } else {
                trimmed.to_owned()
            }
        })
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn query_emacs(shell_pids: &[u32]) -> Option<Vec<EmacsProcess>> {
    let pids = shell_pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let expression = format!(
        r#"(progn
  (require 'json)
  (require 'seq)
  (json-encode
   (delq nil
         (mapcar
          (lambda (pid)
            (let* ((proc (seq-find
                          (lambda (candidate) (equal (process-id candidate) pid))
                          (process-list)))
                   (buffer (and proc (process-buffer proc))))
              (when (buffer-live-p buffer)
                (list
                 (cons 'shell_pid pid)
                 (cons 'emacs_pid (emacs-pid))
                 (cons 'frames
                       (vconcat
                        (delete-dups
                         (mapcar
                          (lambda (window)
                            (frame-parameter (window-frame window) 'name))
                          (get-buffer-window-list buffer nil t)))))
                 (cons 'perspectives
                       (vconcat
                        (if (and (fboundp 'persp-persps)
                                 (fboundp 'persp-buffers)
                                 (fboundp 'safe-persp-name))
                            (mapcar
                             #'safe-persp-name
                             (seq-filter
                              (lambda (perspective)
                                (memq buffer (persp-buffers perspective)))
                              (delq nil (persp-persps))))
                          nil))))))
          '({pids})))))"#
    );
    let output = capture_emacs(&expression)?;
    parse_emacs_output(&output)
}

fn parse_emacs_output(output: &str) -> Option<Vec<EmacsProcess>> {
    let encoded: String = serde_json::from_str(output.trim()).ok()?;
    serde_json::from_str(&encoded).ok()
}

fn capture_emacs(expression: &str) -> Option<String> {
    let mut child = Command::new("emacsclient")
        .args(["--alternate-editor=false", "--eval", expression])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut output = String::new();
                child.stdout.take()?.read_to_string(&mut output).ok()?;
                return Some(output);
            }
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() < EMACS_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[derive(Deserialize)]
struct HyprlandClient {
    pid: u32,
    #[serde(default = "default_true")]
    mapped: bool,
    #[serde(default)]
    title: String,
    workspace: HyprlandWorkspace,
}

#[derive(Deserialize)]
struct HyprlandWorkspace {
    name: String,
}

const fn default_true() -> bool {
    true
}

fn workspace_labels() -> BTreeMap<String, String> {
    if let Ok(value) = std::env::var("AGENT_SESSION_STATUS_WORKSPACE_NAMES")
        && let Ok(labels) = serde_json::from_str(&value)
    {
        return labels;
    }

    let path = std::env::var_os("IRONBAR_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(default_ironbar_config);
    let Ok(contents) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(config) = serde_json::from_str::<Value>(&contents) else {
        return BTreeMap::new();
    };

    let mut labels = BTreeMap::new();
    collect_name_maps(&config, &mut labels);
    labels
}

fn default_ironbar_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config")
        })
        .join("ironbar/config.json")
}

fn collect_name_maps(value: &Value, labels: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("workspaces")
                && let Some(name_map) = object.get("name_map").and_then(Value::as_object)
            {
                for (name, label) in name_map {
                    if let Some(label) = label.as_str() {
                        labels
                            .entry(name.clone())
                            .or_insert_with(|| label.to_owned());
                    }
                }
            }
            for child in object.values() {
                collect_name_maps(child, labels);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_name_maps(child, labels);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn window(title: &str, workspace: &str) -> WindowLocation {
        WindowLocation {
            title: title.to_owned(),
            workspace: workspace.to_owned(),
        }
    }

    fn client(pid: u32, title: &str, workspace: &str) -> HyprlandClient {
        HyprlandClient {
            pid,
            mapped: true,
            title: title.to_owned(),
            workspace: HyprlandWorkspace {
                name: workspace.to_owned(),
            },
        }
    }

    #[test]
    fn nearest_window_ancestor_wins() {
        let workspaces = WorkspaceMap {
            by_pid: BTreeMap::from([
                (10, vec![window("outer", "one")]),
                (20, vec![window("nearest", "two")]),
            ]),
        };

        let (index, windows) = workspaces.for_ancestors(&[30, 20, 10]).unwrap();
        assert_eq!(index, 1);
        assert_eq!(unique_workspaces(windows), ["two"]);
        assert_eq!(workspaces.for_ancestors(&[30, 40]), None);
    }

    #[test]
    fn duplicate_client_pid_on_different_workspaces_is_ambiguous() {
        let labels = BTreeMap::new();
        let first = WorkspaceMap::from_clients(
            vec![
                client(10, "project@emacs", "2"),
                client(10, "agenda", "special"),
            ],
            &labels,
        );
        let reversed = WorkspaceMap::from_clients(
            vec![
                client(10, "agenda", "special"),
                client(10, "project@emacs", "2"),
            ],
            &labels,
        );

        for workspaces in [first, reversed] {
            let (_, windows) = workspaces.for_ancestors(&[30, 10]).unwrap();
            assert_eq!(unique_workspaces(windows), ["2", "special"]);
        }
    }

    #[test]
    fn duplicate_client_pid_on_one_workspace_is_unambiguous() {
        let workspaces = WorkspaceMap::from_clients(
            vec![client(10, "one", "2"), client(10, "two", "2")],
            &BTreeMap::new(),
        );

        let (_, windows) = workspaces.for_ancestors(&[30, 10]).unwrap();
        assert_eq!(unique_workspaces(windows), ["2"]);
    }

    struct MockResolver {
        calls: Cell<usize>,
        responses: Vec<EmacsProcess>,
    }

    impl EmacsResolver for MockResolver {
        fn query(&self, _shell_pids: &[u32]) -> Vec<EmacsProcess> {
            self.calls.set(self.calls.get() + 1);
            self.responses.clone()
        }
    }

    fn pending_emacs() -> PendingEmacsLocation {
        PendingEmacsLocation {
            session_pid: 30,
            shell_pid: 20,
            emacs_pid: 10,
            windows: vec![
                window("project@emacs", "2"),
                window("**Agenda**", "special:orgmode"),
            ],
        }
    }

    #[test]
    fn disabled_emacs_resolution_never_invokes_resolver() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: Vec::new(),
        };

        assert!(resolve_emacs_locations(&[pending_emacs()], false, &resolver).is_empty());
        assert_eq!(resolver.calls.get(), 0);
    }

    #[test]
    fn enabled_emacs_resolution_without_ambiguous_sessions_never_invokes_resolver() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: Vec::new(),
        };

        assert!(resolve_emacs_locations(&[], true, &resolver).is_empty());
        assert_eq!(resolver.calls.get(), 0);
    }

    #[test]
    fn visible_emacs_frame_resolves_its_unique_hyprland_workspace() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: vec![EmacsProcess {
                shell_pid: 20,
                emacs_pid: 10,
                frames: vec!["project@emacs".to_owned()],
                perspectives: vec!["~/project/".to_owned()],
            }],
        };

        assert_eq!(
            resolve_emacs_locations(&[pending_emacs()], true, &resolver).get(&30),
            Some(&SessionLocation::Wayland("2".to_owned()))
        );
        assert_eq!(resolver.calls.get(), 1);
    }

    #[test]
    fn hidden_emacs_buffer_resolves_to_its_perspective() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: vec![EmacsProcess {
                shell_pid: 20,
                emacs_pid: 10,
                frames: Vec::new(),
                perspectives: vec!["~/work/project-copy/".to_owned()],
            }],
        };

        assert_eq!(
            resolve_emacs_locations(&[pending_emacs()], true, &resolver).get(&30),
            Some(&SessionLocation::Emacs("~/work/project-copy".to_owned()))
        );
    }

    #[test]
    fn visible_unmatched_frame_does_not_claim_a_hidden_perspective() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: vec![EmacsProcess {
                shell_pid: 20,
                emacs_pid: 10,
                frames: vec!["renamed@emacs".to_owned()],
                perspectives: vec!["~/project/".to_owned()],
            }],
        };

        assert!(resolve_emacs_locations(&[pending_emacs()], true, &resolver).is_empty());
    }

    #[test]
    fn response_from_another_emacs_process_is_ignored() {
        let resolver = MockResolver {
            calls: Cell::new(0),
            responses: vec![EmacsProcess {
                shell_pid: 20,
                emacs_pid: 99,
                frames: vec!["project@emacs".to_owned()],
                perspectives: Vec::new(),
            }],
        };

        assert!(resolve_emacs_locations(&[pending_emacs()], true, &resolver).is_empty());
    }

    #[test]
    fn parses_emacsclient_quoted_json_output() {
        let output = r#""[{\"shell_pid\":20,\"emacs_pid\":10,\"frames\":[\"project@emacs\"],\"perspectives\":[]}]""#;

        let responses = parse_emacs_output(output).unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].shell_pid, 20);
        assert_eq!(responses[0].frames, ["project@emacs"]);
    }

    #[test]
    fn reads_workspace_labels_from_ironbar_modules() {
        let config = serde_json::json!({
            "start": [{
                "type": "workspaces",
                "name_map": {"1": "0: rocket", "2": "1: browser"}
            }]
        });
        let mut labels = BTreeMap::new();
        collect_name_maps(&config, &mut labels);

        assert_eq!(labels.get("1").map(String::as_str), Some("0: rocket"));
        assert_eq!(labels.get("2").map(String::as_str), Some("1: browser"));
    }
}
