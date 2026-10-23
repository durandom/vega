//! Disambiguate a relative path that matches several repo files, using what
//! is visible on the pane: agents and compilers print paths relative to some
//! subdirectory, and that subdirectory is usually mentioned close above.

use std::cmp::Reverse;
use std::path::Path;

/// Repo-relative files whose path ends with `suffix` at a component boundary.
pub fn suffix_matches(suffix: &str, files: &[String]) -> Vec<String> {
    let suffix = suffix.trim_start_matches("./");
    let tail = format!("/{suffix}");
    files
        .iter()
        .filter(|f| f.as_str() == suffix || f.ends_with(&tail))
        .cloned()
        .collect()
}

/// How strongly the screen points at a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Evidence {
    None,
    /// Its directory, e.g. `plugins/open/` in a tree or a `cd` line.
    Prefix,
    /// The whole path, e.g. `plugins/open/src/main.rs:42`.
    Full,
}

/// Sort `candidates` (repo-relative, all ending in `suffix`) best first.
/// Stronger evidence wins; on a tie the mention closest above the selection
/// (the last line containing `suffix`) wins; then alphabetical.
pub fn rank(mut candidates: Vec<String>, suffix: &str, screen: &str, root: &Path) -> Vec<String> {
    let suffix = suffix.trim_start_matches("./");
    let lines: Vec<&str> = screen.lines().collect();
    let anchor = lines
        .iter()
        .rposition(|l| l.contains(suffix))
        .unwrap_or(lines.len());
    let key = |c: &String| {
        let (evidence, distance) = best_mention(c, suffix, &lines, anchor, root);
        (Reverse(evidence), distance)
    };
    candidates.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.cmp(b)));
    candidates
}

fn best_mention(
    candidate: &str,
    suffix: &str,
    lines: &[&str],
    anchor: usize,
    root: &Path,
) -> (Evidence, usize) {
    let prefix = &candidate[..candidate.len() - suffix.len()];
    let abs = root.join(candidate).display().to_string();
    let abs_prefix = format!("{}/", root.join(prefix.trim_end_matches('/')).display());
    let mut best = (Evidence::None, usize::MAX);
    for (i, line) in lines.iter().enumerate() {
        let evidence = if contains_path(line, candidate) || line.contains(&abs) {
            Evidence::Full
        } else if !prefix.is_empty() && (contains_path(line, prefix) || line.contains(&abs_prefix))
        {
            Evidence::Prefix
        } else {
            continue;
        };
        // Above the selection reads as context; below is a weak signal.
        let distance = if i <= anchor {
            anchor - i
        } else {
            i - anchor + lines.len()
        };
        if evidence > best.0 || (evidence == best.0 && distance < best.1) {
            best = (evidence, distance);
        }
    }
    best
}

/// `needle` occurs in `line` and does not continue a longer path to its
/// left, so `vega/` matches `└── vega/` but not `durandom/vega/`.
fn contains_path(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(i, _)| {
        line[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || "/.-_~".contains(c)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const MAINS: &[&str] = &[
        "plugins/mux/src/main.rs",
        "plugins/open/src/main.rs",
        "shims/herdr/src/main.rs",
        "vega/src/main.rs",
    ];

    #[test]
    fn suffix_matches_whole_components_only() {
        let all = files(&[
            "src/main.rs",
            "a/src/main.rs",
            "a/xsrc/main.rs",
            "src/main.rsx",
        ]);
        assert_eq!(
            suffix_matches("./src/main.rs", &all),
            files(&["src/main.rs", "a/src/main.rs"])
        );
    }

    #[test]
    fn tree_listing_prefers_the_nearest_directory_above() {
        let screen = "vega/\n└── plugins/open/\n    ├── Cargo.toml\n    └── src/main.rs  ← CLI";
        let ranked = rank(files(MAINS), "src/main.rs", screen, Path::new("/r"));
        assert_eq!(ranked[0], "plugins/open/src/main.rs");
        assert_eq!(ranked[1], "vega/src/main.rs");
    }

    #[test]
    fn full_path_beats_a_nearer_prefix() {
        let screen = "see plugins/mux/src/main.rs:42\nshims/herdr/ is unrelated\nsrc/main.rs";
        let ranked = rank(files(MAINS), "src/main.rs", screen, Path::new("/r"));
        assert_eq!(ranked[0], "plugins/mux/src/main.rs");
    }

    #[test]
    fn absolute_mentions_count_and_partial_dirs_do_not() {
        let screen = "cwd /r/shims/herdr/\n~/src/durandom/vega/ is the repo\nsrc/main.rs";
        let ranked = rank(files(MAINS), "src/main.rs", screen, Path::new("/r"));
        assert_eq!(ranked[0], "shims/herdr/src/main.rs");
        // "durandom/vega/" must not count as a mention of the "vega/" crate.
        assert_eq!(
            &ranked[1..],
            &files(&[
                "plugins/mux/src/main.rs",
                "plugins/open/src/main.rs",
                "vega/src/main.rs"
            ])
        );
    }

    #[test]
    fn no_evidence_keeps_alphabetical_order() {
        let ranked = rank(files(&["b/x.rs", "a/x.rs"]), "x.rs", "", Path::new("/r"));
        assert_eq!(ranked, files(&["a/x.rs", "b/x.rs"]));
    }
}
