mod alert;
mod assets;
mod config;
mod event;
mod model;
mod process;
mod render;
mod snapshot;
mod store;
mod workspace;

use std::io::{self, Read};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use crate::alert::{deliver_loaded, synthetic_session};
use crate::assets::{foreground_color, provider_asset};
use crate::config::LoadedConfig;
use crate::event::apply_event;
use crate::model::Provider;
use crate::render::{OutputFormat, provider_status_color, render};
use crate::snapshot::{Snapshot, apply_snapshot_at};
use crate::store::Store;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Override the state directory (primarily useful for tests).
    #[arg(long, global = true, env = "AGENT_SESSION_STATUS_STATE_DIR")]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Ingest one provider lifecycle event as JSON from stdin.
    Event {
        #[arg(value_enum)]
        provider: ProviderArg,
    },
    /// Replace remote sessions from a source snapshot read from JSON stdin.
    Snapshot,
    /// Render the current state once.
    Render {
        #[arg(long, value_enum, default_value_t = FormatArg::Ironbar)]
        format: FormatArg,
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Only include sessions from this source.
        #[arg(long)]
        source: Option<String>,
        /// Aggregate the selected source into one bar segment.
        #[arg(long, requires = "source")]
        group_source: bool,
        /// Omit the provider name when a bar displays a provider image.
        #[arg(long)]
        hide_provider: bool,
        /// Resolve Emacs-hosted sessions to visible workspaces or perspectives.
        #[arg(long, env = "AGENT_SESSION_STATUS_EMACS")]
        emacs: bool,
    },
    /// Stream a new rendered line whenever session state changes.
    Watch {
        #[arg(long, value_enum, default_value_t = FormatArg::Ironbar)]
        format: FormatArg,
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Only include sessions from this source.
        #[arg(long)]
        source: Option<String>,
        /// Aggregate the selected source into one bar segment.
        #[arg(long, requires = "source")]
        group_source: bool,
        /// Omit the provider name when a bar displays a provider image.
        #[arg(long)]
        hide_provider: bool,
        /// Resolve Emacs-hosted sessions to visible workspaces or perspectives.
        #[arg(long, env = "AGENT_SESSION_STATUS_EMACS")]
        emacs: bool,
    },
    /// Exit successfully when at least one session is open.
    Active {
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Only include sessions from this source.
        #[arg(long)]
        source: Option<String>,
    },
    /// Remove all tracked state.
    Clear,
    /// Send a synthetic OpenCode-ready alert using the configured delivery methods.
    AlertTest,
    /// Print the theme-appropriate image asset path for a provider.
    Asset {
        #[arg(value_enum)]
        provider: ProviderArg,
        /// Recolor a locally resolved SVG to match the provider's current status.
        #[arg(long, conflicts_with = "foreground_color")]
        status_color: bool,
        /// Recolor a locally resolved SVG to match Ironbar's foreground color.
        #[arg(long, conflicts_with = "status_color")]
        foreground_color: bool,
        /// Exact color to replace in a locally resolved SVG.
        #[arg(long)]
        source_color: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderArg {
    Opencode,
    Claude,
    Codex,
}

impl From<ProviderArg> for Provider {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::Opencode => Self::OpenCode,
            ProviderArg::Claude => Self::Claude,
            ProviderArg::Codex => Self::Codex,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FormatArg {
    Ironbar,
    Waybar,
    Details,
    Popup,
    Json,
}

impl From<FormatArg> for OutputFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Ironbar => Self::Ironbar,
            FormatArg::Waybar => Self::Waybar,
            FormatArg::Details => Self::Details,
            FormatArg::Popup => Self::Popup,
            FormatArg::Json => Self::Json,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(&cli.command, Command::AlertTest) {
        let loaded = LoadedConfig::load()?;
        if !loaded.config.idle_alerts.notification && !loaded.config.idle_alerts.sound {
            bail!("idle alerts are disabled; enable notification and/or sound in config.json");
        }
        return deliver_loaded(&[synthetic_session()], &loaded);
    }
    if let Command::Asset {
        provider,
        status_color,
        foreground_color: use_foreground_color,
        source_color,
    } = &cli.command
    {
        let provider = (*provider).into();
        let color = if *status_color {
            let store = Store::new(cli.state_dir.clone())?;
            Some(provider_status_color(&store.load_and_prune()?, provider))
        } else if *use_foreground_color {
            Some(foreground_color())
        } else {
            None
        };
        if source_color.is_some() != color.is_some() {
            bail!("--source-color and a tint option must be used together");
        }
        println!(
            "{}",
            provider_asset(provider, source_color.as_deref(), color.as_deref())?.display()
        );
        return Ok(());
    }
    let store = Store::new(cli.state_dir)?;

    match cli.command {
        Command::Event { provider } => {
            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .context("failed to read event JSON from stdin")?;
            let payload = serde_json::Value::from_str(&input).context("invalid event JSON")?;
            let transitions = store.update(|state, _| {
                apply_event(state, provider.into(), &payload);
                Ok(())
            })?;
            deliver_ingestion_alerts(&transitions);
        }
        Command::Snapshot => {
            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .context("failed to read snapshot JSON from stdin")?;
            let snapshot =
                serde_json::from_str::<Snapshot>(&input).context("invalid snapshot JSON")?;
            let transitions =
                store.update(|state, timestamp| apply_snapshot_at(state, snapshot, timestamp))?;
            deliver_ingestion_alerts(&transitions);
        }
        Command::Render {
            format,
            provider,
            source,
            group_source,
            hide_provider,
            emacs,
        } => {
            let state = store.load_and_prune()?;
            println!(
                "{}",
                render(
                    &state,
                    format.into(),
                    provider.map(Into::into),
                    source.as_deref(),
                    group_source,
                    !hide_provider,
                    emacs,
                )?
            );
        }
        Command::Watch {
            format,
            provider,
            source,
            group_source,
            hide_provider,
            emacs,
        } => store.watch(
            format.into(),
            provider.map(Into::into),
            source,
            group_source,
            !hide_provider,
            emacs,
        )?,
        Command::Active { provider, source } => {
            let state = store.load_and_prune()?;
            let provider = provider.map(Into::into);
            let active = state.sessions.values().any(|session| {
                provider.is_none_or(|provider| session.provider == provider)
                    && source
                        .as_ref()
                        .is_none_or(|source| session.source == *source)
                    && (session.kind == crate::model::SessionKind::Main
                        || session.effective_status() != crate::model::Status::Idle)
            });
            if !active {
                bail!("no active sessions");
            }
        }
        Command::Clear => store.clear()?,
        Command::AlertTest => unreachable!(),
        Command::Asset { .. } => unreachable!(),
    }

    Ok(())
}

fn deliver_ingestion_alerts(transitions: &[crate::model::Session]) {
    let result = LoadedConfig::load().and_then(|config| deliver_loaded(transitions, &config));
    if let Err(error) = result {
        eprintln!("agent-session-status: warning: idle alert failed: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emacs_resolution_is_opt_in_for_render_and_watch() {
        for command in ["render", "watch"] {
            let cli = Cli::try_parse_from(["agent-session-status", command, "--emacs"]).unwrap();
            let enabled = match cli.command {
                Command::Render { emacs, .. } | Command::Watch { emacs, .. } => emacs,
                _ => false,
            };
            assert!(enabled);
        }
    }
}
