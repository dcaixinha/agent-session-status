use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

use crate::process::ancestor_pids;

#[derive(Default)]
pub struct WorkspaceMap {
    by_pid: BTreeMap<u32, String>,
}

impl WorkspaceMap {
    pub fn detect() -> Self {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            return Self::from_hyprland();
        }
        Self::default()
    }

    pub fn for_process(&self, pid: u32) -> Option<&str> {
        self.for_ancestors(&ancestor_pids(pid))
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
        Self {
            by_pid: clients
                .into_iter()
                .filter(|client| client.mapped)
                .map(|client| {
                    let name = client.workspace.name;
                    let label = labels.get(&name).cloned().unwrap_or(name);
                    (client.pid, label)
                })
                .collect(),
        }
    }

    fn for_ancestors(&self, ancestors: &[u32]) -> Option<&str> {
        ancestors
            .iter()
            .find_map(|pid| self.by_pid.get(pid).map(String::as_str))
    }
}

#[derive(Deserialize)]
struct HyprlandClient {
    pid: u32,
    #[serde(default = "default_true")]
    mapped: bool,
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
    use super::*;

    #[test]
    fn nearest_window_ancestor_wins() {
        let workspaces = WorkspaceMap {
            by_pid: BTreeMap::from([(10, "one".to_owned()), (20, "two".to_owned())]),
        };

        assert_eq!(workspaces.for_ancestors(&[30, 20, 10]), Some("two"));
        assert_eq!(workspaces.for_ancestors(&[30, 40]), None);
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
