//! Host-specific service endpoints: environment, then a local first-line file.
//! No estate addresses are compiled in. See docs/design/config.md.
use std::path::Path;

pub fn get(variable: &str, file: &str) -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")));
    resolve(
        std::env::var(variable).ok().as_deref(),
        config.as_deref(),
        file,
    )
}

pub fn resolve(value: Option<&str>, config: Option<&Path>, file: &str) -> Option<String> {
    if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
        return Some(v.trim().to_string());
    }
    let text = std::fs::read_to_string(config?.join("vega").join(file)).ok()?;
    text.lines()
        .next()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_require_explicit_configuration_and_environment_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = Some(dir.path());
        for file in ["nats.url", "bd.remote", "bd.sql", "janitor-embed.url"] {
            assert_eq!(resolve(None, root, file), None);
            std::fs::create_dir_all(dir.path().join("vega")).unwrap();
            let path = dir.path().join("vega").join(file);
            std::fs::write(&path, "  service.example:1234  \n# comment\n").unwrap();
            assert_eq!(
                resolve(None, root, file).as_deref(),
                Some("service.example:1234")
            );
            assert_eq!(
                resolve(Some(""), root, file).as_deref(),
                Some("service.example:1234")
            );
            assert_eq!(
                resolve(Some("override.example:5678"), root, file).as_deref(),
                Some("override.example:5678")
            );
            std::fs::write(&path, "  \n").unwrap();
            assert_eq!(resolve(None, root, file), None);
        }
        assert_eq!(resolve(None, None, "nats.url"), None);
    }
}
