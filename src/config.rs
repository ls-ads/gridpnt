//! Connections: the one thing (besides colours) a user configures.
//!
//! They live in `connections.ini` in gridpnt's config directory:
//!
//! ```ini
//! [work]
//! type = databricks
//! profile = DEFAULT          ; a profile in ~/.databrickscfg, or use host =
//! warehouse = 1234567890abcdef
//! env = prod                 ; shown permanently in the status line
//! pin = main.sales, main.finance, analytics
//!
//! [scratch]
//! type = sqlite
//! path = /home/me/scratch.db
//! ```
//!
//! gridpnt never stores a password or token here. Credentials come from the
//! database vendor's own tooling.

use std::path::PathBuf;

/// One `[section]` of an INI file and its `key = value` lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Parse INI text. This small format is shared with `~/.databrickscfg`.
/// Lines starting with `#` or `;` are comments, as is ` ;` or ` #` after a
/// value. Lines before the first section are ignored.
pub fn parse_ini(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            sections.push(Section {
                name: name.trim().to_string(),
                entries: Vec::new(),
            });
        } else if let (Some((key, value)), Some(section)) = (line.split_once('='), sections.last_mut()) {
            let value = value.split(" ;").next().unwrap_or(value);
            let value = value.split(" #").next().unwrap_or(value);
            section.entries.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    sections
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    /// A label such as `prod` or `dev`, always visible while connected.
    pub env: Option<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Sqlite { path: PathBuf },
    Databricks(Databricks),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Databricks {
    /// Workspace URL. If absent it comes from the profile or environment.
    pub host: Option<String>,
    /// Profile name in `~/.databrickscfg`.
    pub profile: Option<String>,
    /// SQL warehouse id that runs the queries.
    pub warehouse: String,
    /// The only parts of the catalog gridpnt will list: `catalog` or
    /// `catalog.schema`. Empty means everything.
    pub pins: Vec<String>,
}

/// Turn parsed sections into connections.
pub fn connections(sections: &[Section]) -> Result<Vec<Connection>, String> {
    sections.iter().map(connection).collect()
}

fn connection(section: &Section) -> Result<Connection, String> {
    let name = &section.name;
    let required = |key: &str| {
        section
            .get(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("connection [{name}] needs `{key} = ...`"))
    };
    let optional = |key: &str| section.get(key).filter(|v| !v.is_empty()).map(str::to_string);

    let kind = match required("type")? {
        "sqlite" => Kind::Sqlite {
            path: PathBuf::from(required("path")?),
        },
        "databricks" => Kind::Databricks(Databricks {
            host: optional("host"),
            profile: optional("profile"),
            warehouse: required("warehouse")?.to_string(),
            pins: section
                .get("pin")
                .unwrap_or("")
                .split(',')
                .map(|pin| pin.trim().to_string())
                .filter(|pin| !pin.is_empty())
                .collect(),
        }),
        other => return Err(format!("connection [{name}] has unknown type `{other}`")),
    };
    Ok(Connection {
        name: name.clone(),
        env: optional("env"),
        kind,
    })
}

/// The user's home directory, from the environment.
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where `connections.ini` lives: `%APPDATA%\gridpnt` on Windows,
/// `$XDG_CONFIG_HOME/gridpnt` or `~/.config/gridpnt` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".config")))
    };
    base.map(|dir| dir.join("gridpnt"))
}

/// Where caches live: `%LOCALAPPDATA%\gridpnt` on Windows,
/// `$XDG_CACHE_HOME/gridpnt` or `~/.cache/gridpnt` elsewhere.
pub fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".cache")))
    };
    base.map(|dir| dir.join("gridpnt"))
}

/// Read every connection the user has defined. A missing file is not an
/// error; it just means there are none yet.
pub fn load_connections() -> Result<Vec<Connection>, String> {
    let Some(path) = config_dir().map(|dir| dir.join("connections.ini")) else {
        return Ok(Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => connections(&parse_ini(&text)).map_err(|e| format!("{}: {e}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}
