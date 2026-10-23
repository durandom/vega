//! Local herdr aliases to registry hosts. Published remote paths are never opened.
use serde_json::Value;
use std::collections::BTreeSet;

pub fn key(host: &str) -> String {
    host.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A saved label's SSH target identifies its registry host. Missing, malformed or
/// conflicting mappings are unknown; a registry host is not itself a machine label.
pub fn registry_for_machine(label: &str, machines: &Value, local: &str) -> Option<String> {
    if key(label) == local {
        return Some(local.into());
    }
    let mut hosts = BTreeSet::new();
    for m in machines.as_array()? {
        if m["label"].as_str()?.to_lowercase() != label.to_lowercase() {
            continue;
        }
        let target = m["target"].as_str()?.trim_start_matches("ssh://");
        let target = target.rsplit('@').next()?;
        if target.starts_with('[') {
            return None;
        }
        let host = target.split(':').next()?;
        if host.is_empty() {
            return None;
        }
        hosts.insert(key(host));
    }
    (hosts.len() == 1)
        .then(|| hosts.into_iter().next())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saved_machine_edges_preserve_unknown_instead_of_guessing() {
        let machines = json!([
            {"label":"ipv6", "target":"ssh://user@[::1]:22"},
            {"label":"io", "target":"user@io-wsl:22"},
            {"label":"other", "target":"other-host"},
            {"label":"missing"}
        ]);
        assert_eq!(registry_for_machine("ipv6", &machines, "proteus"), None);
        assert_eq!(
            registry_for_machine("io", &machines, "proteus"),
            Some("io-wsl".into())
        );
        assert_eq!(
            registry_for_machine("other", &machines, "proteus"),
            Some("other-host".into())
        );
        assert_eq!(registry_for_machine("missing", &machines, "proteus"), None);
        assert_eq!(registry_for_machine("absent", &machines, "proteus"), None);
        assert_eq!(
            registry_for_machine("Proteus", &Value::Null, "proteus"),
            Some("proteus".into())
        );
        assert_eq!(
            registry_for_machine(
                "io",
                &json!([
                    {"label":"io", "target":"first"}, {"label":"IO", "target":"second"}
                ]),
                "proteus"
            ),
            None
        );
    }
}
