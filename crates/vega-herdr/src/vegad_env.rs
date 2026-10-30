//! The unquoted KEY=value environment pinned when vegad is installed.
use std::{env, fs, path::PathBuf};

pub fn read() -> Vec<(String, String)> {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/state"))
        });
    state
        .and_then(|p| fs::read_to_string(p.join("vega/vegad.env")).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
