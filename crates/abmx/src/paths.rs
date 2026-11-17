use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// `--config` → `$ABMX_CONFIG` → `$XDG_CONFIG_HOME/abmx/config.toml` →
/// `~/.config/abmx/config.toml` → `./abmx.toml`.
///
/// The order mirrors `abmx-parser`'s parser discovery: explicit first, then user
/// config, then the working directory, and an error that lists what was tried
/// instead of silently using a default.
pub fn config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.exists() {
            bail!("config {} does not exist", path.display());
        }
        return Ok(path.to_path_buf());
    }

    if let Some(path) = std::env::var_os("ABMX_CONFIG") {
        let path = PathBuf::from(path);
        if !path.exists() {
            bail!(
                "ABMX_CONFIG points at {}, which does not exist",
                path.display()
            );
        }
        return Ok(path);
    }

    let candidates = vec![
        xdg_config_dir().join("abmx/config.toml"),
        PathBuf::from("abmx.toml"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    let tried = candidates
        .iter()
        .map(|candidate| format!("  {}", candidate.display()))
        .collect::<Vec<_>>()
        .join("\n");

    bail!(
        "no config found; tried:\n{tried}\n\n\
         Run `abmx setup` for the interactive quickstart, or `abmx init` to write \
         a starter config."
    )
}

pub fn xdg_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

pub fn xdg_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/state"))
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_state_db() -> PathBuf {
    xdg_state_dir().join("abmx/state.db")
}

/// The user-level parser corpus. `abmx parser init` writes here when no
/// `--parsers` directory is given.
pub fn default_parsers_dir() -> PathBuf {
    xdg_config_dir().join("abmx/parsers")
}

pub fn default_token_path() -> PathBuf {
    xdg_state_dir().join("abmx/gmail-credentials.json")
}

/// Expand a leading `~/` because TOML config files are written by humans.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

pub fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    let expanded = expand_tilde(path);
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

/// Read a secret from an environment variable, a literal, or a file.
///
/// Precedence is env → literal → file so systemd's `EnvironmentFile` and
/// `LoadCredential` can override a config file that is checked into git.
pub fn resolve_secret(
    env: &str,
    literal: Option<&str>,
    file: Option<&Path>,
) -> Result<Option<String>> {
    if let Ok(value) = std::env::var(env) {
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }

    if let Some(value) = literal {
        if !value.is_empty() {
            return Ok(Some(value.to_owned()));
        }
    }

    if let Some(path) = file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading secret {}", path.display()))?;
        return Ok(Some(text.trim().to_owned()));
    }

    Ok(None)
}
