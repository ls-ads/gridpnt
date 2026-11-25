//! Finding the workspace address and a token for it.
//!
//! gridpnt stores no credentials of its own. It looks, in order, at:
//!
//! 1. the connection's own `host`, else the `host` of its profile in
//!    `~/.databrickscfg`, else `DATABRICKS_HOST`;
//! 2. for the token: the profile's `token`, else `DATABRICKS_TOKEN`, else
//!    the Databricks CLI (`databricks auth token`), which covers browser
//!    (OAuth) logins whose tokens live in the operating system keychain.
//!
//! A token is only ever used with the host it was configured for, so a
//! token for one workspace is never sent to another.

use std::process::Command;

use serde::Deserialize;

use crate::backend::Result;
use crate::config::{self, Section};

/// Where the bearer token comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// A personal access token found in a profile or the environment.
    Fixed(String),
    /// Ask the Databricks CLI each time one is needed. These tokens expire
    /// after about an hour, so the client asks again when one is rejected.
    Cli { host: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Workspace URL with scheme and no trailing slash.
    pub host: String,
    pub source: TokenSource,
}

/// Work out credentials from the real environment and `~/.databrickscfg`.
pub fn from_system(connection: &config::Databricks) -> Result<Credentials> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let file = var("DATABRICKS_CONFIG_FILE")
        .map(Into::into)
        .or_else(|| config::home_dir().map(|home| home.join(".databrickscfg")));
    let text = file.and_then(|path| std::fs::read_to_string(path).ok());
    let profiles = config::parse_ini(text.as_deref().unwrap_or(""));
    resolve(connection, &var, &profiles)
}

/// The same decision with its inputs passed in, so it can be tested.
pub fn resolve(
    connection: &config::Databricks,
    var: &dyn Fn(&str) -> Option<String>,
    profiles: &[Section],
) -> Result<Credentials> {
    let profile_name = connection
        .profile
        .clone()
        .or_else(|| var("DATABRICKS_CONFIG_PROFILE"))
        .unwrap_or_else(|| "DEFAULT".into());
    let profile = profiles.iter().find(|section| section.name == profile_name);
    if profile.is_none() && connection.profile.is_some() {
        return Err(format!("profile [{profile_name}] was not found in .databrickscfg"));
    }

    let profile_host = profile.and_then(|p| p.get("host")).map(normalise_host).transpose()?;
    let env_host = var("DATABRICKS_HOST").map(|h| normalise_host(&h)).transpose()?;
    let host = match &connection.host {
        Some(host) => normalise_host(host)?,
        None => profile_host
            .clone()
            .or_else(|| env_host.clone())
            .ok_or("no workspace address: set `host` on the connection or in a .databrickscfg profile")?,
    };

    let profile_token = profile
        .filter(|_| profile_host.as_ref() == Some(&host))
        .and_then(|p| p.get("token"))
        .filter(|token| !token.is_empty());
    // An environment token with no DATABRICKS_HOST beside it is taken to
    // be meant for whichever workspace is being opened.
    let env_token = var("DATABRICKS_TOKEN").filter(|_| env_host.as_ref().is_none_or(|h| *h == host));

    let source = match (profile_token, env_token) {
        (Some(token), _) => TokenSource::Fixed(token.to_string()),
        (None, Some(token)) => TokenSource::Fixed(token),
        (None, None) => TokenSource::Cli { host: host.clone() },
    };
    Ok(Credentials { host, source })
}

/// Tidy a workspace address: add `https://`, drop a trailing slash. Plain
/// `http://` is refused except for this machine, because a token sent over
/// it could be read by anyone on the network.
pub fn normalise_host(host: &str) -> Result<String> {
    let host = host.trim().trim_end_matches('/');
    if let Some(rest) = host.strip_prefix("http://") {
        let name = rest.split([':', '/']).next().unwrap_or("");
        return if name == "127.0.0.1" || name == "localhost" {
            Ok(host.to_string())
        } else {
            Err(format!("refusing to use {host}: it is not https"))
        };
    }
    if host.is_empty() {
        return Err("workspace address is empty".into());
    }
    if host.starts_with("https://") {
        Ok(host.to_string())
    } else {
        Ok(format!("https://{host}"))
    }
}

impl TokenSource {
    /// Produce a token, running the Databricks CLI if that is the source.
    pub fn token(&self) -> Result<String> {
        match self {
            TokenSource::Fixed(token) => Ok(token.clone()),
            TokenSource::Cli { host } => cli_token(host),
        }
    }

    pub fn can_refresh(&self) -> bool {
        matches!(self, TokenSource::Cli { .. })
    }
}

#[derive(Deserialize)]
struct CliToken {
    access_token: String,
}

fn cli_token(host: &str) -> Result<String> {
    let how_to_fix = format!(
        "no token for {host}. Either put one in ~/.databrickscfg, set DATABRICKS_TOKEN, \
         or install the Databricks CLI and run: databricks auth login --host {host}"
    );
    let output = Command::new("databricks")
        .args(["auth", "token", "--host", host])
        .output()
        .map_err(|_| how_to_fix.clone())?;
    if !output.status.success() {
        return Err(how_to_fix);
    }
    serde_json::from_slice::<CliToken>(&output.stdout)
        .map(|parsed| parsed.access_token)
        .map_err(|_| "the Databricks CLI did not return a token".to_string())
}
