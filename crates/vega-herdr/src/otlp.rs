//! Shared opt-in OTLP endpoint configuration for vega exporters.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// OTLP/HTTP base URL, without a signal path such as `/v1/logs`.
    pub endpoint: String,
    pub timeout: Duration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    enabled: bool,
    endpoint: Option<String>,
    timeout_ms: Option<u64>,
}

/// Read `$XDG_CONFIG_HOME/vega/otlp.toml` (or `~/.config/vega/otlp.toml`).
/// A missing file and `enabled = false` both mean that export is off.
pub fn load() -> Result<Option<Config>, String> {
    let Some(path) = config_path() else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let config: FileConfig = toml_edit::de::from_str(&text)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    if !config.enabled {
        return Ok(None);
    }
    let endpoint = config
        .endpoint
        .filter(|endpoint| !endpoint.trim().is_empty())
        .ok_or_else(|| format!("{}: enabled OTLP config requires endpoint", path.display()))?;
    let parsed = url::Url::parse(&endpoint)
        .map_err(|error| format!("{}: invalid OTLP endpoint: {error}", path.display()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!(
            "{}: OTLP endpoint must be an absolute HTTP(S) base URL",
            path.display()
        ));
    }
    let timeout_ms = config.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    if timeout_ms == 0 {
        return Err(format!(
            "{}: timeout_ms must be greater than zero",
            path.display()
        ));
    }
    Ok(Some(Config {
        endpoint: endpoint.trim_end_matches('/').to_string(),
        timeout: Duration::from_millis(timeout_ms),
    }))
}

fn config_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(root.join("vega/otlp.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static ENV: Mutex<()> = Mutex::new(());

    fn with_config(text: Option<&str>) -> Result<Option<Config>, String> {
        let _guard = ENV.lock().unwrap();
        let root = tempdir().unwrap();
        let dir = root.path().join("vega");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = text {
            std::fs::write(dir.join("otlp.toml"), text).unwrap();
        }
        let old = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", root.path());
        let result = load();
        match old {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        result
    }

    #[test]
    fn missing_and_disabled_config_are_off() {
        assert_eq!(with_config(None).unwrap(), None);
        assert_eq!(with_config(Some("enabled = false\n")).unwrap(), None);
    }

    #[test]
    fn enabled_config_reads_base_and_timeout() {
        assert_eq!(
            with_config(Some(
                "enabled = true\nendpoint = 'http://localhost:4318/'\ntimeout_ms = 1200\n"
            ))
            .unwrap(),
            Some(Config {
                endpoint: "http://localhost:4318".into(),
                timeout: Duration::from_millis(1200),
            })
        );
    }

    #[test]
    fn enabled_requires_valid_http_endpoint() {
        assert!(with_config(Some("enabled = true\n")).is_err());
        assert!(with_config(Some("enabled = true\nendpoint = 'ftp://host'\n")).is_err());
        assert!(with_config(Some(
            "enabled = true\nendpoint = 'http://host'\ntimeout_ms = 0\n"
        ))
        .is_err());
    }
}
