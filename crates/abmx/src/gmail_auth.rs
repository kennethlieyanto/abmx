//! The one OAuth flow shared by `abmx auth login` and `abmx setup`.

use std::path::{Path, PathBuf};

use abmx_gmail::{authorize, authorize_manual, OAuthConfig, TokenStore};
use anyhow::{bail, Result};

use crate::config::Config;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Print the URL instead of trying to open a browser.
    pub no_browser: bool,
    /// Pin the loopback port; `None` uses the config, then `0` (any port).
    pub port: Option<u16>,
    /// Approve anywhere and paste the callback URL back.
    pub manual: bool,
}

/// Run consent and store the refresh token. Returns the token path.
pub fn authorize_gmail(config: &Config, base: &Path, options: &Options) -> Result<PathBuf> {
    if config.fetch.gmail.is_none() {
        bail!("no [fetch.gmail] section in the config");
    }

    let credentials = config.gmail_credentials(base)?;

    let mut oauth = OAuthConfig::new(credentials.client_id);
    oauth.client_secret = credentials.client_secret;
    oauth.port = options.port.unwrap_or_else(|| config.gmail_oauth_port());
    oauth.open_browser = !options.no_browser && !options.manual;

    let credentials = if options.manual {
        authorize_manual(&oauth)?
    } else {
        authorize(&oauth)?
    };

    let store = TokenStore::new(config.gmail_token_path(base));
    store.save(&credentials)?;
    Ok(store.path().to_path_buf())
}
