#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${HOME}/.local/bin
data_dir=${XDG_DATA_HOME:-${HOME}/.local/share}/agent-session-status
opencode_plugin_dir=${XDG_CONFIG_HOME:-${HOME}/.config}/opencode/plugins

cargo build --locked --release --manifest-path "${repo_dir}/Cargo.toml"
mkdir -p "${bin_dir}" "${data_dir}" "${opencode_plugin_dir}"
ln -sf "${repo_dir}/target/release/agent-session-status" "${bin_dir}/agent-session-status"
ln -sf "${repo_dir}/assets/opencode-logo-light-square.svg" \
  "${data_dir}/opencode-logo-light-square.svg"
ln -sf "${repo_dir}/assets/opencode-logo-dark-square.svg" \
  "${data_dir}/opencode-logo-dark-square.svg"
ln -sf "${repo_dir}/assets/agent-complete.wav" \
  "${data_dir}/agent-complete.wav"
ln -sf "${repo_dir}/assets/LICENSE.OpenCode" \
  "${data_dir}/LICENSE.OpenCode"
ln -sf "${repo_dir}/integrations/opencode/agent-session-status.ts" \
  "${opencode_plugin_dir}/agent-session-status.ts"

printf '%s\n' \
  "Installed agent-session-status in ${bin_dir}." \
  "Installed assets in ${data_dir}." \
  "Installed the OpenCode plugin in ${opencode_plugin_dir}." \
  "" \
  "Manual configuration remains:" \
  "  Ironbar: examples/ironbar-module.json and examples/ironbar-style.css" \
  "  Waybar:  examples/waybar-module.json and examples/waybar-style.css" \
  "  Claude:  integrations/claude/settings.json" \
  "  Codex:   integrations/codex/hooks.json (then review it with /hooks)"
