use std::fs;

use crate::model::Provider;

pub fn provider_process(provider: Provider) -> Option<(u32, u64)> {
    let mut pid = std::process::id();

    while pid > 1 {
        let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        if matches_provider(provider, &comm) {
            return process_start(pid).map(|start| (pid, start));
        }
        pid = parent_pid(pid)?;
    }
    None
}

pub fn is_same_process(pid: u32, expected_start: Option<u64>) -> bool {
    let Some(actual_start) = process_start(pid) else {
        return false;
    };
    expected_start.is_none_or(|expected| expected == actual_start)
}

pub fn ancestor_pids(mut pid: u32) -> Vec<u32> {
    let mut ancestors = Vec::new();
    while pid > 1 {
        ancestors.push(pid);
        let Some(parent) = parent_pid(pid) else {
            break;
        };
        pid = parent;
    }
    ancestors
}

fn matches_provider(provider: Provider, comm: &str) -> bool {
    match provider {
        Provider::OpenCode => comm == "opencode",
        Provider::Claude => comm == "claude" || comm == "claude.exe",
        Provider::Codex => comm == "codex" || comm == "codex-cli",
    }
}

fn parent_pid(pid: u32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

fn process_start(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The process name is parenthesized and can contain spaces. Field 22 is
    // the 20th whitespace-delimited field after the closing parenthesis.
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
