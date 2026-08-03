use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};

use crate::model::Provider;

static ASSET_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn provider_asset(
    provider: Provider,
    source_color: Option<&str>,
    target_color: Option<&str>,
) -> Result<PathBuf> {
    let theme = active_theme();
    let filename = asset_filename(provider, theme);
    let source = custom_asset(provider, theme)?
        .or_else(|| find_data_file(&filename))
        .or_else(|| development_asset(&filename).filter(|path| path.is_file()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "could not find {filename}; set {} or install it in an XDG data directory",
                asset_env_name(provider, None)
            )
        })?;

    match (source_color, target_color) {
        (Some(source_color), Some(target_color)) => {
            tinted_asset(&source, source_color, target_color)
        }
        (None, None) => Ok(source),
        _ => bail!("--source-color and a tint option must be used together"),
    }
}

fn custom_asset(provider: Provider, theme: &str) -> Result<Option<PathBuf>> {
    for name in [
        asset_env_name(provider, Some(theme)),
        asset_env_name(provider, None),
    ] {
        let Some(value) = std::env::var_os(&name) else {
            continue;
        };
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            bail!("{name} must be an absolute path");
        }
        if !path.is_file() {
            bail!("{name} does not point to a file: {}", path.display());
        }
        return Ok(Some(path));
    }
    Ok(None)
}

fn asset_env_name(provider: Provider, theme: Option<&str>) -> String {
    let provider = provider.key().to_ascii_uppercase();
    theme.map_or_else(
        || format!("AGENT_SESSION_STATUS_ASSET_{provider}"),
        |theme| {
            format!(
                "AGENT_SESSION_STATUS_ASSET_{}_{}",
                provider,
                theme.to_ascii_uppercase()
            )
        },
    )
}

pub fn foreground_color() -> String {
    std::env::var("AGENT_SESSION_STATUS_COLOR_FOREGROUND")
        .ok()
        .or_else(|| {
            fs::read_to_string(active_stylesheet())
                .ok()
                .and_then(|stylesheet| defined_color(&stylesheet, "fg"))
        })
        .unwrap_or_else(|| match active_theme() {
            "dark" => "#ffffff".to_owned(),
            _ => "#000000".to_owned(),
        })
}

fn defined_color(stylesheet: &str, name: &str) -> Option<String> {
    stylesheet.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next(), parts.next()) {
            (Some("@define-color"), Some(candidate), Some(value)) if candidate == name => {
                Some(value.trim_end_matches(';').to_owned())
            }
            _ => None,
        }
    })
}

fn tinted_asset(source: &Path, source_color: &str, target_color: &str) -> Result<PathBuf> {
    if source.extension().and_then(|extension| extension.to_str()) != Some("svg") {
        bail!("tinting requires an SVG asset: {}", source.display());
    }
    let escaped_color = escape_svg_attribute(target_color);
    let source_content = fs::read_to_string(source)
        .with_context(|| format!("failed to read {}", source.display()))?;
    if !source_content.contains(source_color) {
        bail!(
            "source color {source_color} was not found in {}",
            source.display()
        );
    }
    let mut hasher = DefaultHasher::new();
    source_content.hash(&mut hasher);
    source_color.hash(&mut hasher);
    target_color.hash(&mut hasher);
    let source_hash = hasher.finish();
    let content = source_content.replace(source_color, &escaped_color);
    let filename = format!("asset-{source_hash:016x}.svg");
    let dir = asset_cache_dir()?;
    let output = dir.join(filename);
    let _guard = ASSET_WRITE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if output.exists() {
        return Ok(output);
    }
    let temporary = output.with_extension(format!("svg.{}.tmp", std::process::id()));
    fs::write(&temporary, content)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    fs::rename(&temporary, &output)
        .with_context(|| format!("failed to replace {}", output.display()))?;
    Ok(output)
}

fn asset_cache_dir() -> Result<PathBuf> {
    let uid = fs::metadata("/proc/self")?.uid();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let (root, name) = if let Some(runtime) = runtime {
        (runtime, "agent-session-status")
    } else {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home_dir().join(".cache"));
        if !cache.is_absolute() {
            bail!("XDG_RUNTIME_DIR is unavailable and no absolute cache directory exists");
        }
        (cache, "agent-session-status")
    };
    fs::create_dir_all(&root)?;
    let application = root.join(name);
    match fs::DirBuilder::new().mode(0o700).create(&application) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&application)?;
    if !metadata.is_dir() || metadata.uid() != uid {
        bail!(
            "runtime directory is not a private user directory: {}",
            application.display()
        );
    }
    fs::set_permissions(&application, fs::Permissions::from_mode(0o700))?;
    let assets = application.join("assets");
    fs::create_dir_all(&assets)?;
    Ok(assets)
}

fn escape_svg_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn asset_filename(provider: Provider, theme: &str) -> String {
    match provider {
        Provider::OpenCode => format!("opencode-logo-{theme}-square.svg"),
        Provider::Claude => format!("claude-logo-{theme}-square.svg"),
        Provider::Codex => format!("codex-logo-{theme}-square.svg"),
    }
}

fn active_theme() -> &'static str {
    match std::env::var("AGENT_SESSION_STATUS_THEME")
        .unwrap_or_else(|_| "auto".to_owned())
        .to_lowercase()
        .as_str()
    {
        "dark" => "dark",
        "light" => "light",
        _ if active_stylesheet_is_dark() => "dark",
        _ => "light",
    }
}

fn active_stylesheet_is_dark() -> bool {
    fs::canonicalize(active_stylesheet())
        .unwrap_or_default()
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_lowercase().contains("dark"))
}

fn active_stylesheet() -> PathBuf {
    std::env::var_os("IRONBAR_CSS")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("ironbar/style.css"))
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

pub(crate) fn data_dir() -> PathBuf {
    data_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(|| home_dir().join(".local/share/agent-session-status"))
}

pub(crate) fn find_data_file(filename: &str) -> Option<PathBuf> {
    data_dirs()
        .into_iter()
        .map(|directory| directory.join(filename))
        .find(|path| path.is_file())
}

fn data_dirs() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let user = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home_dir().join(".local/share"));
    directories.push(user.join("agent-session-status"));

    let system = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    directories.extend(
        std::env::split_paths(&system)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("agent-session-status")),
    );
    directories
}

pub(crate) fn development_asset(filename: &str) -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let executable = fs::canonicalize(executable).ok()?;
    executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|root| root.join("assets").join(filename))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_provider_assets() {
        assert_eq!(
            asset_filename(Provider::OpenCode, "dark"),
            "opencode-logo-dark-square.svg"
        );
        assert_eq!(
            asset_filename(Provider::Claude, "light"),
            "claude-logo-light-square.svg"
        );
        assert_eq!(
            asset_filename(Provider::Codex, "dark"),
            "codex-logo-dark-square.svg"
        );
    }

    #[test]
    fn maps_provider_asset_environment_names() {
        assert_eq!(
            asset_env_name(Provider::Claude, Some("dark")),
            "AGENT_SESSION_STATUS_ASSET_CLAUDE_DARK"
        );
        assert_eq!(
            asset_env_name(Provider::Codex, None),
            "AGENT_SESSION_STATUS_ASSET_CODEX"
        );
    }

    #[test]
    fn generic_tint_requires_svg_and_replaces_only_the_explicit_color() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("custom.svg");
        fs::write(&source, "<svg><path fill='#123456'/></svg>").unwrap();

        let output = tinted_asset(&source, "#123456", "#abcdef").unwrap();
        assert!(fs::read_to_string(output).unwrap().contains("#abcdef"));
        assert!(tinted_asset(&source, "#missing", "#abcdef").is_err());

        let png = temp.path().join("custom.png");
        fs::write(&png, b"not an image").unwrap();
        assert!(tinted_asset(&png, "#123456", "#abcdef").is_err());
    }

    #[test]
    fn escapes_colors_for_svg_attributes() {
        assert_eq!(
            escape_svg_attribute("red\" onload=\"bad"),
            "red&quot; onload=&quot;bad"
        );
    }

    #[test]
    fn reads_named_css_color() {
        assert_eq!(
            defined_color("@define-color fg #eee8d5; /* base2 */", "fg"),
            Some("#eee8d5".to_owned())
        );
    }
}
