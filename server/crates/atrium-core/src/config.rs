//! Bootstrap configuration: `/etc/atrium/core.toml`.
//!
//! Only what cannot come from the database (ADR-012): where to listen, where
//! state lives, how much to log, and the server's display name. Everything
//! else is state. The file is root-owned and read-only to Core; Core also
//! validates it, and a file that fails validation stops startup rather than
//! being quietly corrected. Unknown keys are refused for the same reason — a
//! misspelt setting that is silently ignored is a setting that silently does
//! not apply.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

use crate::fsio::{self, ReadError};
use crate::layout::Layout;

/// Upper bound on the file's size.
const CONFIG_LIMIT: u64 = 64 * 1024;
/// The port from ADR-015 when none is given.
pub const DEFAULT_LISTEN: &str = "[::]:7443";
/// Longest accepted server name.
const NAME_LIMIT: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    server_name: String,
    listen: Option<String>,
    data_dir: Option<PathBuf>,
    log_level: Option<String>,
}

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// The default.
    Info,
    /// Verbose.
    Debug,
    /// Very verbose.
    Trace,
}

impl LogLevel {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => return None,
        })
    }

    /// The `tracing` filter directive.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// Validated bootstrap configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// What the owner calls this server. Also what `atriumctl` asks the
    /// operator to type before a destructive console action.
    pub server_name: String,
    /// Where the API will listen (from M1D).
    pub listen: SocketAddr,
    /// The state directory.
    pub data_dir: PathBuf,
    /// Log verbosity.
    pub log_level: LogLevel,
}

/// Why the configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// There is no `core.toml`.
    Missing,
    /// It cannot be read, or is not a regular file.
    Unreadable(String),
    /// It is not valid TOML, or has an unknown key.
    Syntax(String),
    /// A value is out of range.
    Invalid(&'static str, String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("core.toml does not exist"),
            Self::Unreadable(why) => write!(formatter, "core.toml {why}"),
            Self::Syntax(why) => write!(formatter, "core.toml is not valid: {why}"),
            Self::Invalid(field, why) => write!(formatter, "core.toml: {field} {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Reads and validates `core.toml`.
///
/// # Errors
///
/// A [`ConfigError`]; nothing is defaulted around an invalid value.
pub fn load(layout: &Layout) -> Result<Config, ConfigError> {
    let bytes =
        fsio::read_regular(&layout.config(), CONFIG_LIMIT).map_err(|error| match error {
            ReadError::Missing => ConfigError::Missing,
            other => ConfigError::Unreadable(other.to_string()),
        })?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| ConfigError::Syntax("not UTF-8".to_owned()))?;
    parse(text, layout)
}

/// Parses and validates configuration text against `layout`.
///
/// # Errors
///
/// See [`load`].
pub fn parse(text: &str, layout: &Layout) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(text).map_err(|error| {
        // toml's messages quote the offending line; keep only the first line
        // of the message so a long value cannot flood the log.
        ConfigError::Syntax(error.message().lines().next().unwrap_or("").to_owned())
    })?;

    let server_name = raw.server_name.trim().to_owned();
    if server_name.is_empty() || server_name.chars().count() > NAME_LIMIT {
        return Err(ConfigError::Invalid(
            "server_name",
            format!("must be 1 to {NAME_LIMIT} characters"),
        ));
    }
    if server_name.chars().any(char::is_control) {
        return Err(ConfigError::Invalid(
            "server_name",
            "must not contain control characters".to_owned(),
        ));
    }

    let listen_text = raw.listen.as_deref().unwrap_or(DEFAULT_LISTEN);
    let listen: SocketAddr = listen_text
        .parse()
        .map_err(|_| ConfigError::Invalid("listen", "must be an address and port".to_owned()))?;
    if listen.port() <= 1024 {
        return Err(ConfigError::Invalid(
            "listen",
            "must name a port above 1024; Core has no privilege to bind lower".to_owned(),
        ));
    }

    // The state directory is fixed by the plan. The setting exists so the file
    // states it, not so it can move: anything else is refused, not honoured.
    let data_dir = raw
        .data_dir
        .unwrap_or_else(|| layout.state_dir().to_path_buf());
    if data_dir.components().ne(layout.state_dir().components()) {
        return Err(ConfigError::Invalid(
            "data_dir",
            format!("must be {}", layout.state_dir().display()),
        ));
    }

    let log_level = match raw.log_level.as_deref() {
        None => LogLevel::Info,
        Some(text) => LogLevel::parse(text).ok_or(ConfigError::Invalid(
            "log_level",
            "must be one of error, warn, info, debug, trace".to_owned(),
        ))?,
    };

    Ok(Config {
        server_name,
        listen,
        data_dir,
        log_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::production()
    }

    #[test]
    fn a_minimal_file_gets_the_documented_defaults() {
        let config = parse("server_name = \"home\"\n", &layout()).expect("valid");
        assert_eq!(config.server_name, "home");
        assert_eq!(config.listen, DEFAULT_LISTEN.parse().expect("address"));
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/atrium"));
        assert_eq!(config.log_level, LogLevel::Info);
    }

    #[test]
    fn every_field_is_validated() {
        let cases = [
            ("", "missing server_name"),
            ("server_name = \"\"", "empty name"),
            ("server_name = \"a\\u0007b\"", "control character"),
            (
                "server_name = \"h\"\nlisten = \"0.0.0.0:443\"",
                "privileged port",
            ),
            (
                "server_name = \"h\"\nlisten = \"nonsense\"",
                "unparseable listen",
            ),
            (
                "server_name = \"h\"\ndata_dir = \"/tmp/elsewhere\"",
                "moved data_dir",
            ),
            (
                "server_name = \"h\"\ndata_dir = \"/var/lib/atrium/../x\"",
                "dotted data_dir",
            ),
            ("server_name = \"h\"\nlog_level = \"loud\"", "unknown level"),
            (
                "server_name = \"h\"\nlistne = \"[::]:7443\"",
                "misspelt key",
            ),
            ("server_name = [", "not TOML"),
        ];
        for (text, what) in cases {
            assert!(parse(text, &layout()).is_err(), "{what} must be refused");
        }
    }

    #[test]
    fn a_long_name_is_refused() {
        let text = format!("server_name = \"{}\"", "x".repeat(NAME_LIMIT + 1));
        assert!(parse(&text, &layout()).is_err());
        let text = format!("server_name = \"{}\"", "x".repeat(NAME_LIMIT));
        assert!(parse(&text, &layout()).is_ok());
    }

    #[test]
    fn data_dir_follows_a_relocated_tree() {
        let relocated = Layout::under(std::path::Path::new("/tmp/fixture")).expect("valid");
        let config = parse("server_name = \"h\"", &relocated).expect("valid");
        assert_eq!(
            config.data_dir,
            PathBuf::from("/tmp/fixture/var/lib/atrium")
        );
        assert!(parse(
            "server_name = \"h\"\ndata_dir = \"/var/lib/atrium\"",
            &relocated
        )
        .is_err());
    }
}
