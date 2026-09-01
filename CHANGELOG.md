# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- Add user-visible changes below using Keep a Changelog categories. -->

### Added

- Added opt-in `--emacs` and `AGENT_SESSION_STATUS_EMACS` location resolution, showing the Hyprland workspace for visible Emacs frames and the perspective name for hidden buffers.

### Fixed

- Registered the Arch container checkout as a safe Git directory before validating and publishing AUR metadata.
- Prevented multiple Hyprland windows sharing one PID from assigning an arbitrary workspace to every descendant session.

## [0.1.0] - 2026-08-05

### Added

- Event-driven OpenCode, Claude Code, and Codex session tracking with effective `waiting`, `working`, and `idle` states.
- Ironbar labels and popups plus continuous Waybar JSON modules with tooltips and dynamic status classes.
- Multiple main-session aggregation, active subagent counts, provider/source filtering, and grouped remote snapshots.
- Atomic runtime state, PID identity checks, stale/TTL pruning, filesystem-driven updates, and Hyprland workspace association.
- Optional batched idle-transition notifications and completion sounds with scope, subagent, and DND controls.
- Theme-aware OpenCode assets and explicit user-provided provider images with local generic SVG tinting.
- Source-built Arch/AUR packaging with automated validation and release-driven publication.

### Security

- Collision-safe remote identities, strict snapshot validation, escaped untrusted Pango content, private state and tint caches, and atomic cache writes.

[Unreleased]: https://github.com/dcaixinha/agent-session-status/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/dcaixinha/agent-session-status/releases/tag/v0.1.0
