# Contributor Runbook

## Scope and map

`agent-session-status` is a Linux Rust CLI that ingests local provider lifecycle events and vendor-neutral remote snapshots, persists compact runtime state, renders Ironbar and Waybar output, and optionally delivers exact idle-transition alerts.

- `src/main.rs`: public CLI and command dispatch.
- `src/model.rs`: persisted model, identity keys, and effective status.
- `src/event.rs`: OpenCode/Claude/Codex event adapters.
- `src/snapshot.rs`: external snapshot boundary.
- `src/store.rs`: locking, atomic state, pruning, transition detection, watch.
- `src/render.rs`: visibility, aggregation, formats, Pango, colors/icons.
- `src/assets.rs`: custom/XDG image lookup, theme/foreground detection, explicit tint cache.
- `src/config.rs` and `src/alert.rs`: alert configuration, filtering, and external command delivery.
- `src/process.rs` and `src/workspace.rs`: Linux process identity and Hyprland association.
- `integrations/`: provider-owned plugin/hook entry points; `examples/`: Ironbar and Waybar samples; `assets/`: licensed OpenCode images and sound.

## Core invariants

- Status priority is `waiting > working > idle`. A nonempty pending set always makes effective status `waiting`.
- Session keys must isolate provider identities. Remote keys must also isolate source and instance without delimiter collisions.
- Main sessions remain visible while idle. Idle subagents are hidden; active subagents render as `+N` and are removed with their root where lifecycle data permits.
- Provider aggregation uses effective states. Uniform main sessions render `(N)`; mixed main sessions render the count at the displayed highest main state over total. Active subagents can raise segment color priority but must not replace an existing main-session display state.
- Waybar output is one compact JSON object per physical line. Its primary class is the highest effective visible state; `mixed` describes differing visible main states, and `empty` keeps a watcher alive without visible text.
- The store must remain lock-protected and atomic. Preserve read/prune/update/prune/write ordering and compute alerts only from sessions present in both transaction states.
- PID liveness must compare Linux process-start ticks, not PID alone. PID-less local state has the 24-hour limit; imported state uses TTL.
- `watch` must retain filesystem-driven updates and periodic refresh so TTL/stale pruning appears during quiet periods.

## Provider and remote boundaries

- Provider adapters consume their documented event shapes, tolerate irrelevant valid events, and normalize into the shared model. Preserve old persisted-field defaults when a concrete shipped-state compatibility need exists.
- Integration snippets are fragments. Documentation and installers must instruct users to merge hook arrays, never overwrite unrelated provider configuration.
- Snapshot input requires source, TTL, instances, and complete session fields. Validate nonblank source/IDs, duplicate instances, duplicate `(provider, session ID)` pairs, enum values, and expiry overflow before mutation.
- A present session array authoritatively replaces one instance; an empty array clears it; omitted/null sessions retain state and expiry while updating the label; omitted instances are removed for that source.
- Imported sessions are main sessions with explicit effective status and no PID, pending set, subagent tree, or workspace association.
- Keep the remote boundary vendor-neutral. Core accepts snapshots produced elsewhere; it does not own remote discovery or transport.
- Never embed proprietary remote discovery, network, authentication, credential, host-inventory, or provider-installation logic in core. Such logic belongs in an external producer and must communicate only through the public snapshot schema.

## Alerts

- Defaults disable notification and sound. Config is strict when present, while missing fields retain defaults.
- Eligible transitions are effective `working|waiting -> idle` for the same key within one locked event/snapshot transaction. New idle sessions, repeats, removals, clear, expiry, pruning, and snapshot omission are silent.
- Filter scope and subagent policy after transition detection. Batch all eligible transitions from one transaction into at most one notification and one sound.
- Ingestion commits before delivery and treats config/delivery failures as warnings. `alert-test` is intentionally strict and synthetic; it does not test transition detection.
- DND configuration is argv, never shell text. Preserve affirmative-output handling, the two-second timeout, and fail-closed sound suppression.
- Tests must use mock runners or disabled delivery and must not emit actual notifications or sounds.

## Public interfaces and trust

- Treat CLI names/options, output formats, environment variables, config JSON, snapshot JSON, persisted state, installer destinations, integration filenames, and example commands as public interfaces. Update docs and tests with intentional changes.
- Prefer minimal schema changes. Add compatibility behavior only for concrete persisted/shipped use, not speculation.
- Escape session paths, labels, and discovered data before Pango rendering. `AGENT_SESSION_STATUS_SOURCE_LABEL` is the explicit trusted-Pango exception; do not broaden that trust boundary.
- Keep XDG roles separate: config under `XDG_CONFIG_HOME`, user assets under `XDG_DATA_HOME`, packaged assets under `XDG_DATA_DIRS`, and state/tint cache under `XDG_RUNTIME_DIR` or their documented fallbacks.

## Assets and installation

- Theme detection is based on the canonical active CSS target filename containing `dark`; foreground detection parses `@define-color fg`.
- Bundle only byte-exact upstream OpenCode SVGs with their upstream MIT notice. Do not add Claude or OpenAI artwork, transformed provider marks, or vendor-specific tint colors to the public repository.
- Custom asset overrides must be absolute files. Theme-specific overrides win over provider-wide overrides, which win over XDG and development lookup. Tinting is generic, SVG-only, and requires an explicit source color supplied by the user; cache identity must include source content and both colors. Cache writes must remain atomic; when XDG runtime storage is unavailable, use the private user XDG cache directory.
- Keep licensing explicit: code is MIT, the OpenCode assets retain upstream MIT, and `agent-complete.wav` is CC0 1.0. User-provided assets are outside the project license and documentation must assign rights compliance to the user.
- `install.sh` creates symlinks into the checkout. The checkout must remain in place, and docs must state that warning.
- Do not run the installer merely to verify a change: it mutates the user's live binary, data, and OpenCode plugin symlinks. Build and test through Cargo instead unless installer behavior itself is under explicit test.

## Verification gates

Run before handing off Rust or behavior changes:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --release --all-features
```

For CLI/docs changes, compare every documented command and option against `cargo run --quiet -- --help` and each subcommand's help. Validate JSON examples with a parser, shell with a POSIX shell syntax check where applicable, relative Markdown paths, screenshot dimensions/checksums when copied, and forbidden terminology requested by the task.

Automated coverage includes reducers, snapshots, store transitions/pruning, rendering, Waybar watch output, assets, workspace mapping, and mocked alert delivery. Live provider hooks/plugins, Ironbar rendering, Waybar/Pango/CSS rendering, watcher behavior across filesystems, desktop notifications, audio servers, DND tools, Hyprland IPC, and font/theme integration remain manual gaps.

## Style

- Rust: stable edition 2024, formatted with rustfmt, warning-free under the repository clippy command. Prefer small functions, explicit errors at input boundaries, deterministic ordered collections where output/state stability matters, and comments only for non-obvious invariants.
- TypeScript: follow the integration's existing semicolon-free concise style, retain type imports, serialize only relevant events, and await child completion.
- JSON: valid strict JSON, two-space indentation, no comments; examples must distinguish merge fragments from complete files.
- POSIX shell: `/bin/sh`, `set -eu`, quoted expansions, portable commands, and no shell-specific arrays or conditionals.

## Worktree safety

- Inspect before editing and keep changes within the requested scope.
- Never discard, overwrite, stage, or reformat unrelated user/agent changes. Adapt around concurrent edits; stop only for a direct conflict.
- Never use destructive Git commands. Do not commit, amend, push, or create a pull request unless explicitly requested.
- Do not touch live dotfiles/configuration during development or verification. Use temporary XDG/state directories for tests that exercise paths.
